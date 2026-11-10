//! Plug links, node actions, variable stores and state begin.

use super::*;

/// Actions that change nothing the server tracks: the client runs the same
/// graphs and plays effects, records stats and shows its own messages itself.
/// The server only follows their out plug. The list is the Python reference's
/// (`no_op` in `ow174/game/script/nodes.py`).
const SERVER_NO_OPS: &[&str] = &[
    "STUStatescriptActionEffect",
    "STUStatescriptActionCriteria",
    "STU_CEBDAF17", // TrackStat
    "STU_100EF8C2", // SendClientOnlyGameMessage
    "STUStatescriptActionSendClientGlobalGameMessage",
    "STUStatescriptActionPlayScript",
    "STU_7106C356", // unnamed, listed by the Python reference
    "STU_C43F9944", // unnamed, listed by the Python reference
];

/// Unnamed actions the server runs as no-ops without knowing what they do,
/// as the Python reference did for every node it had no model for. Each use is
/// recorded as an [`Assumption`].
const ASSUMED_NO_OPS: &[&str] = &[
    "STU_E9694EFD", // Soldier: 76's sprint, graph 0255 node 40; hashed fields only
    "STU_6E5C5F66", // melee, graph 0043 node 44; no fields
    "STU_1C2CC6D5", // melee, graph 0043 node 8; one unnamed field
];

/// Melee's states (graph 0043): health change, knockback and four unnamed
/// ones. The client reads them as on/off only ("none" codec); the server
/// doesn't run their effect yet, like the Python VM, so melee hits nothing.
const ASSUMED_INERT_STATES: &[&str] = &[
    "STU_23A00A53",
    "STUStatescriptStateModifyHealth",
    "STU_0A6BE10E",
    "STUStatescriptStatePhysicsExplosion",
    "STU_B5C87F3D",
    "STU_54FEB21C",
];

impl<'graph, 'world, 'machine, W: World> Vm<'graph, 'world, 'machine, W> {
    pub(super) fn follow(
        &mut self,
        id: u16,
        node: &Node,
        path: &str,
        caller: Option<StateKey>,
        depth: usize,
    ) -> Result<bool, InitFault> {
        if depth >= MAX_PLUG_DEPTH {
            return Err(InitFault::PlugDepth);
        }
        self.spend("plug traversal")?;
        let mut taken = false;
        let graph = self.graph(id);
        for (position, input) in node.links(path) {
            self.spend("plug target")?;
            let Some(position) = position else {
                // The reference treats an unresolved plug as taken.
                taken = true;
                continue;
            };
            let Some(target) = graph.node(position) else {
                self.report(self.site(id, node), Issue::MissingNode(position));
                taken = true;
                continue;
            };
            if target.client_only {
                continue;
            }
            if target.is_state() {
                if let Some(raw) = target.state {
                    let key = StateKey { instance: id, raw };
                    if target.class == "STU_01178155" && input == "m_inPlug" {
                        let state = self.machine.snapshot.instances[&id]
                            .states
                            .entry(raw)
                            .or_insert_with(|| State::dormant(StateKind::ActiveQuery));
                        let output = if state.active {
                            "m_7273C81B"
                        } else {
                            "m_C6F3744E"
                        };
                        self.follow(id, target, output, Some(key), depth + 1)?;
                        taken = true;
                        continue;
                    }
                    if target.class == "STU_7B9E8909" && input == "m_inPlug" {
                        // The bridge consumes its input even when inactive. Only an
                        // explicit begin request can open it.
                        let state = self.machine.snapshot.instances[&id]
                            .states
                            .entry(raw)
                            .or_insert_with(|| State::dormant(StateKind::Plain));
                        if state.active {
                            self.follow(id, target, "m_outPlug", Some(key), depth + 1)?;
                        }
                        taken = true;
                        continue;
                    }
                    if target.class == "STUStatescriptStateAbility" && input == "m_3B7BED2E" {
                        self.stop_ability(key, target, depth + 1)?;
                        taken = true;
                        continue;
                    }
                    let event = if input == ABORT_PLUG {
                        Event::Abort(raw)
                    } else {
                        Event::Begin(raw)
                    };
                    self.post(id, event)?;
                    taken = true;
                }
            } else {
                let caller = if target.is_action() { None } else { caller };
                taken = self.run_node(id, target, caller, depth + 1)? || taken;
            }
        }
        Ok(taken)
    }

    pub(super) fn run_node(
        &mut self,
        id: u16,
        node: &Node,
        caller: Option<StateKey>,
        depth: usize,
    ) -> Result<bool, InitFault> {
        self.spend("node execution")?;
        match node.class.as_str() {
            _ if is_entry(node) => self.follow(id, node, "m_outPlug", None, depth),
            "STUStatescriptActionSetVar" => {
                if !self.store(id, node, &node.fields, true)? {
                    return Ok(false);
                }
                self.follow(id, node, "m_outPlug", None, depth)?;
                Ok(true)
            }
            "STUStatescriptActionSendGameMessage" => {
                if !self.send_game_message(id, node)? {
                    return Ok(false);
                }
                self.follow(id, node, "m_outPlug", None, depth)?;
                Ok(true)
            }
            "STU_22926252"
                if field(&node.fields, "m_67B81D25").is_null()
                    && field(&node.fields, "m_EB35D2F1").is_null() =>
            {
                self.post(id, Event::ExitInstance)?;
                self.follow(id, node, "m_outPlug", None, depth)?;
                Ok(true)
            }
            "STU_12F5D52D" => {
                let mut complete = true;
                for item in array_field(&node.fields, "m_143D4C5B") {
                    if item.is_object() {
                        complete = self.store(id, node, item, false)? && complete;
                    }
                }
                if complete {
                    self.follow(id, node, "m_outPlug", None, depth)?;
                }
                Ok(complete)
            }
            "STU_3387AB5D" => {
                let Some(value) =
                    self.evaluate(id, node, field(&node.fields, "m_condition"), caller)
                else {
                    return Ok(false);
                };
                if !truthy(&value) {
                    return self.follow(id, node, "m_falsePlug", caller, depth);
                }
                Ok(self.follow(id, node, "m_truePlug", caller, depth)?
                    || (node.links("m_truePlug").next().is_none()
                        && node.links("m_falsePlug").next().is_none()))
            }
            "STUStatescriptActionSwitch" => {
                let Some(value) = self.evaluate(id, node, field(&node.fields, "m_262AE982"), None)
                else {
                    return Ok(false);
                };
                let mut path = "m_CD38D5DB".to_owned();
                for (number, case) in array_field(&node.fields, "m_FDD34C25").iter().enumerate() {
                    if !case.is_object() {
                        continue;
                    }
                    let Some(candidate) = self.evaluate(id, node, field(case, "m_value"), None)
                    else {
                        return Ok(false);
                    };
                    if equal(&value, &candidate) {
                        path = format!("m_FDD34C25[{number}]");
                        break;
                    }
                }
                self.follow(id, node, &path, None, depth)?;
                self.follow(id, node, "m_outPlug", None, depth)?;
                Ok(true)
            }
            class if SERVER_NO_OPS.contains(&class) => {
                self.follow(id, node, "m_outPlug", None, depth)?;
                Ok(true)
            }
            class if ASSUMED_NO_OPS.contains(&class) => {
                self.assume(self.site(id, node), class, "no-op");
                self.follow(id, node, "m_outPlug", None, depth)?;
                Ok(true)
            }
            class if self.tolerates_unknowns() => {
                self.assume(self.site(id, node), class, "follows its out plug");
                self.follow(id, node, "m_outPlug", None, depth)?;
                Ok(true)
            }
            _ => {
                self.report(
                    self.site(id, node),
                    Issue::UnsupportedAction(node.class.clone()),
                );
                Ok(false)
            }
        }
    }

    pub(super) fn store(
        &mut self,
        id: u16,
        node: &Node,
        fields: &Json,
        indexed: bool,
    ) -> Result<bool, InitFault> {
        let Some(value) = self.evaluate(id, node, field(fields, "m_value"), None) else {
            return Ok(false);
        };
        let site = self.site(id, node);
        let target = match expr::lvalue(field(fields, "m_out_Var")) {
            Ok(Some(target)) => source_key(id, target),
            Ok(None) => return Ok(true),
            Err(error) => {
                self.report(site, Issue::Expression(error));
                return Ok(false);
            }
        };
        if indexed && !field(fields, "m_key").is_null() {
            self.report(site, Issue::UnsupportedField("SetVar map key"));
            return Ok(false);
        }
        let index = field(fields, "m_index");
        if indexed && !index.is_null() {
            let index = if index.is_object() {
                let Some(value) = self.evaluate(id, node, index, None) else {
                    return Ok(false);
                };
                Some(expr::to_int(&value, 0))
            } else {
                index
                    .as_i64()
                    .or_else(|| index.as_f64().map(|value| value as i64))
                    .or_else(|| index.as_bool().map(i64::from))
                    .or_else(|| index.as_str()?.trim().parse().ok())
            };
            let Some(index) = index else {
                self.report(site, Issue::UnsupportedField("SetVar array index"));
                return Ok(false);
            };
            if let Ok(index) = usize::try_from(index) {
                let mut items = match self.machine.value(target, None) {
                    Value::Array(items) => items,
                    Value::Null => Vec::new(),
                    value => vec![value],
                };
                if index >= usize::from(u16::MAX) {
                    self.report(
                        site,
                        Issue::UnsupportedField("SetVar array exceeds the wire count"),
                    );
                    return Ok(false);
                }
                if index >= items.len() {
                    let extra = index + 1 - items.len();
                    self.machine.work_left =
                        self.machine
                            .work_left
                            .checked_sub(extra)
                            .ok_or(InitFault::WorkBudget {
                                limit: self.machine.max_work,
                                operation: "SetVar array extension",
                            })?;
                    items.resize(index + 1, Value::Null);
                }
                items[index] = value;
                self.write(target, Value::Array(items))?;
                return Ok(true);
            }
        }
        self.write(target, value)?;
        Ok(true)
    }

    pub(super) fn begin(&mut self, key: StateKey, depth: usize) -> Result<(), InitFault> {
        let id = key.instance;
        let Some(node) = self.graph(id).state(usize::from(key.raw)) else {
            self.report(
                Site {
                    entity: self.machine.snapshot.entity,
                    instance: id,
                    graph: self.graph(id).index,
                    node: None,
                },
                Issue::MissingState(key.raw),
            );
            return Ok(());
        };
        if let Some(state) = self.state(key).filter(|state| state.active) {
            let kind = state.kind;
            if configured(node, "m_0DBAFD7F") {
                match kind {
                    StateKind::Wait => {
                        self.machine
                            .queues
                            .retain(id, |_, event| event.state() != Some(key.raw));
                        self.state_mut(key).expect("active wait").start_ms = self.machine.now_ms;
                        self.arm_wait(key, node)?;
                    }
                    StateKind::WaitFrames { .. } => self.arm_frame_wait(key, node)?,
                    _ => (),
                }
            }
            if kind == StateKind::ChaseVar && configured(node, "m_9FF96195") {
                self.finish_chase(key, depth)?;
                self.begin_chase(key, node, depth)?;
            }
            return Ok(());
        }
        let kind = match node.class.as_str() {
            // These client presentation states use the base state lifecycle on the server.
            "STUStatescriptState"
            | "STU_9D7BF987"
            | "STU_B4CD94F4"
            | "STU_90C3BCEA"
            | "STUStatescriptStateDataFlowMapping"
            | "STU_E13B30A8"
            | "STUStatescriptStateUXPresenter" => StateKind::Plain,
            // The authored zero-valued form inherits the base state reader.
            "STU_D9843704"
                if node.is_networked() && node.remote == -1 && {
                    let value = field(&node.fields, "m_0B528B1E");
                    expr::native_shape(value, "STUConfigVarFloat", Some("m_value"))
                        && field(value, "m_value").as_f64() == Some(0.0)
                } =>
            {
                StateKind::Plain
            }
            "STU_BD02E168" if is_client_subgraph_controller(self.graph(id), node) => {
                StateKind::Plain
            }
            // ForCodeSynced publishes to native game-code registration outside
            // the graph. Its VM plugs and empty wire payload use the base state.
            "STU_CD46AF93" if is_networked_for_code_synced(node) => StateKind::Plain,
            // Tips register their state key with native presentation code.
            // The graph still runs the base state's plugs and lifecycle.
            "STU_F5A99B2A"
                if node.is_networked()
                    && field(&node.fields, "m_tips")
                        .as_array()
                        .is_some_and(|tips| {
                            !tips.is_empty()
                                && tips
                                    .iter()
                                    .all(|tip| field(tip, "$").as_str() == Some("STUTip"))
                        }) =>
            {
                StateKind::Plain
            }
            "STU_7B9E8909" => StateKind::Plain,
            "STUStatescriptStateHealthPool" if is_base_health_pool(node) => {
                StateKind::HealthPool { pool: None }
            }
            "STU_E56BA8D8" if node.is_networked() && node.remote == -1 => {
                StateKind::TargetTag { effect: None }
            }
            "STU_066B4605" if self.has_no_ai_control_target(id, node) => StateKind::Plain,
            "STUStatescriptStateBooleanSwitch" => StateKind::BooleanSwitch(false),
            "STU_B591EBFE" => StateKind::DelayedSwitch {
                current: false,
                undecided: true,
            },
            "STU_DEBD057B" => StateKind::Switch { case: None },
            "STU_01178155" => StateKind::ActiveQuery,
            "STUStatescriptStateGamePadVibration" => StateKind::GamePadVibration,
            "STU_37D754C8" | "STUStatescriptStateSubScript" => StateKind::SubScript { child: None },
            "STUStatescriptStateGameMessageEntry" => StateKind::MessageIdle,
            "STUStatescriptStateWait" if !configured(node, "m_BF1A93B0") => StateKind::Wait,
            "STU_D74D4F47" => StateKind::WaitFrames { target: 0 },
            "STU_91EF24A9" => StateKind::AiCandidate { selected: false },
            "STUStatescriptStateLogicalButton" => StateKind::LogicalButton { down: false },
            "STUStatescriptStateStack" | "STU_A97E6999" => StateKind::Stack {
                top: false,
                under: false,
            },
            "STUStatescriptStateAbility" => StateKind::Ability { flags: 0 },
            "STU_316CFEF2" => StateKind::MovementMod,
            "STU_96F1DBCF" if is_owner_movement_sampler(node) => {
                match self.state(key).map(|state| state.kind) {
                    Some(kind @ StateKind::MovementSample(_)) => kind,
                    _ => StateKind::MovementSample(0),
                }
            }
            "STUStatescriptStateAnim" => StateKind::Anim,
            "STU_691BFA55" | "STU_7CA2FBAF" => StateKind::Effect,
            "STU_38EE1100" => StateKind::SendMessage,
            "STUStatescriptStateCosmeticEntity" => StateKind::CosmeticEntity,
            "STUStatescriptStateChaseVar" => StateKind::ChaseVar,
            "STU_2F4E2E3F" => StateKind::Watch,
            "STUStatescriptStateTrackTargets" => StateKind::TrackTargets,
            class if ASSUMED_INERT_STATES.contains(&class) => StateKind::Plain,
            "STU_77E13026" => StateKind::HitList,
            "STUStatescriptStateWeaponVolley" => StateKind::WeaponVolley {
                volleys: 1,
                offset_ms: 0,
                ammo0: 0,
                ammo_var: None,
            },
            class if self.tolerates_unknowns() => {
                self.assume(self.site(id, node), class, "plain state");
                StateKind::Plain
            }
            _ => {
                self.report(
                    self.site(id, node),
                    Issue::UnsupportedState(node.class.clone()),
                );
                return Ok(());
            }
        };
        let previous = self.state(key);
        let activation = previous.map_or(1, |state| state.activation.wrapping_add(1));
        let buffered_press = previous.and_then(|state| state.buffered_press);
        for raw in raw_indices(array_field(&node.fields, "m_exclusiveStateIndices")) {
            self.end(StateKey { instance: id, raw }, false, depth + 1)?;
            self.drop_begin(id, raw);
        }
        self.machine.snapshot.instances[&id].states.insert(
            key.raw,
            State {
                active: true,
                activation,
                start_ms: self.machine.now_ms,
                buffered_press,
                ..State::dormant(kind)
            },
        );
        match kind {
            StateKind::GamePadVibration => self.post(id, Event::Finish(key.raw))?,
            StateKind::Plain if node.class == "STU_066B4605" => {
                // The absent AI relation finishes through the current clock's event queue.
                self.post(id, Event::Finish(key.raw))?;
            }
            StateKind::HealthPool { .. } => self.begin_health_pool(key, node)?,
            StateKind::TargetTag { .. } => self.begin_target_tag(key, node)?,
            StateKind::BooleanSwitch(_) => {
                let Some(value) =
                    self.evaluate(id, node, field(&node.fields, "m_condition"), Some(key))
                else {
                    return Ok(());
                };
                let current = truthy(&value);
                self.state_mut(key).expect("active switch").kind =
                    StateKind::BooleanSwitch(current);
                self.enter_branch(key, node, current, depth)?;
            }
            StateKind::DelayedSwitch { .. } => self.begin_delayed_switch(key, node, depth)?,
            StateKind::Switch { .. } => {
                let Some(case) = self.switch_case(key, node) else {
                    return Ok(());
                };
                self.state_mut(key).expect("active switch").kind = StateKind::Switch { case };
                self.follow(id, node, &case_path(case), Some(key), depth)?;
            }
            StateKind::SubScript { .. } => {
                let child = self.spawn_child(key, node)?;
                self.state_mut(key).expect("active subscript").kind =
                    StateKind::SubScript { child };
            }
            StateKind::MessageIdle => {
                self.follow(id, node, "m_33DA5163", Some(key), depth)?;
            }
            StateKind::Wait => self.arm_wait(key, node)?,
            StateKind::WaitFrames { .. } => self.arm_frame_wait(key, node)?,
            StateKind::AiCandidate { .. } => {
                let site = self.site(id, node);
                if let Some(selected) = self.world.selects_ai_behavior(site) {
                    self.state_mut(key).expect("active AI candidate").kind =
                        StateKind::AiCandidate { selected };
                    if selected {
                        self.follow(id, node, "m_6F92F4BD", Some(key), depth)?;
                    }
                } else {
                    self.report(site, Issue::UnsupportedField("AI behavior selection"));
                }
            }
            StateKind::LogicalButton { .. } => self.enter_button(key, node, depth)?,
            StateKind::Stack { .. } => self.begin_stack(key, node, depth)?,
            StateKind::Ability { .. } => {
                self.restart_cooldown(key, node, depth)?;
                self.register_tick(key, self.machine.now_ms);
            }
            StateKind::MovementSample(previous) => {
                self.update_movement_sample(key, node, previous)?;
                self.register_tick(key, self.machine.now_ms);
            }
            StateKind::Anim => self.arm_animation(key, node)?,
            StateKind::SendMessage => {
                self.send_game_message(id, node)?;
            }
            StateKind::CosmeticEntity => self.begin_cosmetic(key, node, depth)?,
            StateKind::ChaseVar => self.begin_chase(key, node, depth)?,
            StateKind::Watch => {
                let seen = self.evaluate(id, node, field(&node.fields, "m_B7A3CF78"), Some(key));
                self.state_mut(key).expect("active Watch").watch_seen = seen;
            }
            StateKind::WeaponVolley { .. } => self.begin_volley(key, node, depth)?,
            StateKind::TrackTargets => {
                self.assume(self.site(id, node), &node.class, "finds no targets");
            }
            StateKind::HitList => self.assume(self.site(id, node), &node.class, "no hits"),
            StateKind::Plain if ASSUMED_INERT_STATES.contains(&node.class.as_str()) => {
                self.assume(self.site(id, node), &node.class, "no effect");
            }
            StateKind::Plain
            | StateKind::MovementMod
            | StateKind::Effect
            | StateKind::ActiveQuery => {}
        }
        self.follow(id, node, "m_onBeginPlug", Some(key), depth)?;
        self.follow(id, node, "m_subgraphPlug", Some(key), depth)?;
        self.try_complete(key, node, depth)
    }
}
