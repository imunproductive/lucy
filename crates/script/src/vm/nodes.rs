//! Plug links, node actions, variable stores and state begin.
//!
//! Client research (1.74): a plug walk (0x7FF78AA83FA0) follows the links in
//! order. A state target only gets a begin (or abort) request, an event at
//! now, so a state never begins inside a walk; actions and conditions run at
//! once. The server runs every node that is not client-only. A link enters a
//! state through the base plug enter (0x7FF78AA83D80): the abort plug asks
//! for an abort, others a begin. DoBeginState (0x7FF78AA80790): exclusive
//! states end, OnBegin, OnEnter, m_onBeginPlug, m_subgraphPlug, then the
//! completion check (TryComplete 0x7FF78AA81570; base IsComplete
//! 0x7FF78AA83EC0: the first transition plug that takes a link; the state then
//! queues its own abort at now). DoAbortState 0x7FF78AA80480 / DoFinishState
//! 0x7FF78AA80B10: queued events dropped, the subgraph's states (the plug's
//! m_FF3DAF1E list) aborted (0x7FF78AA7FD00), the class's own end, then
//! m_onAbortPlug / m_onFinishedPlug and m_onEndPlug. A state's button edge is
//! vt+0x128 and its output to a linked variable vt+0x188. SetVar
//! (0x7FF78AA7D300): the value (failure: int 0), then a keyed insert, an
//! element write or a plain assignment with the value's own type; then
//! m_outPlug. SetVar and SetVars read their values as assignments (+28, a
//! config list collects into an array: `expr::evaluate_value`). ActiveQuery:
//! a link into its m_inPlug (an STU_A1AAB8C4 input plug) does not begin it;
//! it follows m_7273C81B when the state is on and
//! m_C6F3744E when off (vt+0x198 0x7FF78AAA8920, called by the plug enter for
//! every input plug that is not the abort plug). 01CF st70 is one: on while
//! 01CF waits for 0A43.025, it pulses st108 (the kill feed line) when 003E.025
//! comes.

use super::*;

/// Actions that change nothing the server tracks: the client runs the same
/// graphs and plays effects, records stats and shows its own messages itself.
/// The server only follows their out plug. The list is the Python reference's
/// (`no_op` in `ow174/game/script/nodes.py`) and one from upstream's research.
const SERVER_NO_OPS: &[&str] = &[
    "STUStatescriptActionEffect",
    "STU_100EF8C2", // SendClientOnlyGameMessage
    "STUStatescriptActionSendClientGlobalGameMessage",
    "STUStatescriptActionPlayScript",
    "STU_7106C356", // unnamed, listed by the Python reference
    "STU_C43F9944", // unnamed, listed by the Python reference
    // Records optional trace data, then follows m_outPlug (client function
    // 1B6EBA0; leafall903/ORPHEUS (MIT), commit c239a84, nodes.py). Melee,
    // graph 0043 node 44.
    "STU_6E5C5F66",
    // The base Action: the PC factory builds no class of its own and the
    // inherited handler (1B6EBA0) only records a prediction entry, then
    // follows the empty out plug (leafall903/ORPHEUS (MIT),
    // `wheel_base_action` in nodes.py, `docs/COMMUNICATION_WHEEL.txt`).
    // 05AD node 165 and 023A/02CF/0686's server-only unlock-use reports.
    "STU_631D36C3",
    "STU_94771561",
    // Execute 0x7FF789C3B040: the statescript's frame (+1 when its enum
    // m_93CDFD2F is 1) as seconds (0x7FF789AC7100) raises a time on the
    // entity's component 28 (+0x3A8) and its component 24 (+0x2B0)
    // (0x7FF789B778D0); neither component is networked (no mirror), so the
    // wire never carries it. Ability graphs of
    // Cassidy, Doomfist, Genji, Pharah, Reinhardt, Tracer (16 nodes).
    "STU_6FEE499E",
    // Its own run (0x7FF7898195A0) jumps straight to the base run: the
    // string m_5328E4F8 and value m_07DD813E are never read (69 nodes, the
    // death layer `01CB` rid54/rid55 among them).
    "STU_9F3FE778",
    // A voice category action (`Action_STU_3D419B63` 0x7FF789BC9C70): for
    // each `m_targets` entity it calls the client's voice system (the one
    // `VoiceStimulus` plays through) at vtable +128 with `m_category`, a
    // `079` voice category, or none. Assumption: it stops the target's
    // lines of that category (sound only; `069A` rid7/rid8 run it with
    // category `0005` as an emote begins and ends, `03F2` rid32 for the
    // announcer, `1BEE` rid12). All four uses are networked with a remote
    // index; the server has no voice playing, and its frames don't pulse
    // remote actions outside the wheel's graphs yet.
    "STU_3D419B63",
    // The Workshop Inspector's server-only nodes (no client registration):
    // Enable/Disable Inspector Recording `2425`/`2426` (`STU_B80EB51C`, its
    // bool `m_9A9E72EC`) and Log To Inspector `2E80` (`STU_61975068`). The
    // retail server recorded for the Inspector; ORPHEUS keeps no Inspector
    // (`docs/dev/loose-ends.md`), so they only go on.
    "STU_B80EB51C",
    "STU_61975068",
];

/// Unnamed actions the server runs as no-ops without knowing what they do,
/// as the Python reference did for every node it had no model for. Each use is
/// recorded as an [`Assumption`].
const ASSUMED_NO_OPS: &[&str] = &[
    "STU_E9694EFD", // Soldier: 76's sprint, graph 0255 node 40; hashed fields only
    // The communication wheel's terminal action after each PlayScript
    // (05AD nodes 154 and 155, 02CF node 32): an entity and an integer, no
    // out link.
    "STU_71FE9852",
    // Server-only (no registration of its own: the base action's run):
    // m_D7DDEEAD points for m_target (always the owner), m_instigator
    // optional. 23 sites: 120 or 60 on an ability's use, 40 a second
    // during an ultimate, Genji's deflect (03A3) and Sigma's grasp (1D02)
    // the damage they took from #41 beside its ult charge. Likely the On
    // Fire score, a server rule the server has no model of.
    "STU_D087B558",
    // Play of the Game data: server-only (no registration of its own: the
    // base action's run, `tools/stu.py`), one inline `PlayOfTheGameData`
    // (a `0C2` category, m_id, m_value, m_targets, m_description) and no
    // output variable. 28 nodes in 10 graphs; the body's `01D8` (under
    // `004B`) scores the owner's kills, heals and saves with it, and its
    // st3 waits for the mode's `0694`, which `13C0` holds in the Practice
    // Range. The server plays no Play of the Game, so it keeps no score.
    "STU_0FC6DAEA",
    // The death layer `01CB`'s kill cam (rid38 st21's path, only with a
    // respawn time): server-only actions (no registration of their own).
    // Strong guess from their fields and place: STU_3218BAEB records a reel
    // of m_8796592F (#2441) seconds of m_0FD68554's view into m_D1F6024E
    // (#1374, -1 before); STU_5EAA04BB plays script m_script (#9694) with
    // message 0673 and that reel. The server records no reel: #1374 stays
    // -1, which the camera state STU_D75CDECC then reads.
    "STU_3218BAEB",
    "STU_5EAA04BB",
    // Mode actions with no registration of their own (the base action's
    // run 0x7FF78AA7EBA0: the client follows m_outPlug; leafall903/ORPHEUS
    // (MIT) `rust` `vm/nodes.rs`, the 104319 class registry, checked with
    // `tools/stu.py`). Server-only meaning unknown: STU_88DE7AD3 (one
    // dynamic m_F97A13C6; the phase handoff of 0D0B, 0D76, 0EC2, 0F4A
    // rid57, 0F60), STU_147B34C6 (no fields; 0D76, 180A rid17's phase
    // cleanup), STU_7947FBE4 (no fields; 181D), STU_5B025A5D and
    // STU_8A8255AF (an integer m_5FAC4660, the latter a float m_5CF63914;
    // 0D94).
    "STU_88DE7AD3",
    "STU_147B34C6",
    "STU_7947FBE4",
    "STU_5B025A5D",
    "STU_8A8255AF",
];

/// Melee's states (graph 0043): knockback and three unnamed ones. The client
/// reads them as on/off only ("none" codec); the server doesn't run their
/// effect yet, like the Python VM.
const ASSUMED_INERT_STATES: &[&str] = &[
    "STU_23A00A53",
    "STU_0A6BE10E",
    "STUStatescriptStatePhysicsExplosion",
    "STU_B5C87F3D",
];

/// The player controller's native states (`0C90`): their client codec has
/// no payload (reader 0x7ff78abf5640), and what they do on the server is
/// unknown, so they run as plain states with an assumption. ForProbe
/// (`0C90` st75/st76, server-only: values for native game code), Reloader
/// (`0C90` st106: on a native event it stores the possessed body's place
/// and heading for StateHero and sends `0687` to itself; it never fires
/// here) and StateHero (`0C90` st0, `13C1` st41: the controller's hero
/// body at `0687`'s spawn point after a hero change; a generic state on the
/// client, `RespawnFacing.md`; the server makes and possesses the body
/// itself). HeroSelect runs (`vm/hero_select.rs`).
///
/// The mode's `0CBB` has two more with no registration of their own (the
/// base state: the client does nothing with them), so server-only: `STU_7F488C3E`
/// (st46-st49, no fields, under the phase stacks #19528e setup, #12113e
/// round, #19527e round end, #19526e match end) and `STU_54BBC96C` (st35,
/// no fields, 12 s after a round's end while #8078e is false). What the
/// retail server did with them is not known. Server-only too: the mode
/// graphs' `STU_0FAD550D` (`m_41F2FA6C`, a bool; `0F4A` st34 as a round
/// starts, false, before the ForCodeSynced st33 rows `2271`/`226F`/`2270`).
const ASSUMED_PLAIN_STATES: &[&str] = &[
    "STU_0C3A5D53",
    "STU_AB09B113",
    "STUStatescriptStateHero",
    "STU_7F488C3E",
    "STU_54BBC96C",
    "STU_0FAD550D",
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
                    if target.class == "STU_5E805B5A" && input == "m_F4DD996D" {
                        // The spawn picker consumes its input, and picks
                        // only while active (`vm/entities.rs`).
                        if self.state(key).is_some_and(|state| state.active) {
                            self.pick_spawn(key, target, depth + 1)?;
                        }
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
                    if target.class == "STU_ECAEE401" && input == "m_8CBFEBB9" {
                        // The AI movement state's request input (an
                        // `STU_A1AAB8C4`, client slot 51 `0x7FF789B8A1A0`
                        // hands it to the AI movement component 88): it
                        // reads its move goal again, only while active.
                        if self.state(key).is_some_and(|state| state.active) {
                            self.resolve_ai_move(key, target);
                        }
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
            } else if target.class == "STU_1113EBD5" {
                // A join knows which plug brought each input
                // (`Instance._follow` keeps `plug_source` in upstream's runtime.py).
                taken = self.join(id, target, node.pos, path, caller, depth + 1)? || taken;
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
            // `STU_57692020` sends without `m_targets` (`0686` rid19 sends
            // `0307` after Need Healing): the engine picks the receivers
            // (`send_game_message`).
            "STUStatescriptActionSendGameMessage" | "STU_57692020" => {
                if !self.send_game_message(id, node)? {
                    return Ok(false);
                }
                self.follow(id, node, "m_outPlug", None, depth)?;
                Ok(true)
            }
            "STUStatescriptActionPlayScript" => {
                let graph = self.graph(id).index;
                let played = || {
                    node.field("m_script.m_graph")
                        .and_then(asset_guid)
                        .filter(|&played| played >> 48 == 0x0580)
                        .and_then(|played| u32::try_from(played & 0xFFFF_FFFF_FFFF).ok())
                };
                if let Some(played) = wheel::played_graph(graph, node.pos)
                    .or_else(|| play::server_played_graph(graph, node.pos))
                {
                    self.play_script(id, node, played, play::Plays::Everywhere)?;
                } else if self.graphs.is_aura_graph(graph)
                    || self.graphs.is_workshop_action(graph)
                    || play::HEALTH_PACKS.contains(&graph)
                    || graph == play::BEST_DEFENSE
                {
                    // An aura's graph plays its effect on the body
                    // (`vm/auras.rs`: `0C71` plays `0B99` or `0C72`); a
                    // Workshop action its helper on its targets (Chase
                    // Player Variable At Rate `1CCC` plays `1CCF` on each
                    // player's controller, `vm/workshop.rs`); a health
                    // pack `0046` on itself and `0046` its visual's `0EB9`
                    // ([`play::HEALTH_PACKS`]); Doomfist's passive its
                    // shield grant ([`play::BEST_DEFENSE`]).
                    if let Some(played) = played() {
                        self.play_script(id, node, played, play::Plays::Everywhere)?;
                    }
                } else if self.graphs.ability_button(graph).is_some()
                    && let Some(played) = played()
                {
                    // A hero ability's graph plays its victim's graph on
                    // the bodies it struck (Rocket Punch `0D06` plays
                    // `0D07`, Soundwave `0776` plays `0ECA`): on other
                    // entities only ([`play::Plays::Others`]).
                    self.play_script(id, node, played, play::Plays::Others)?;
                }
                self.follow(id, node, "m_outPlug", None, depth)?;
                Ok(true)
            }
            // A PlayScript start entry: [`Self::play_script`] binds its
            // parameters before it runs.
            "STU_BB4AC733" => self.follow(id, node, "m_outPlug", None, depth),
            // The emote's end (069A nodes 7 and 8, category 5): retire the
            // 1040 helper, then follow the out plug (upstream
            // `finish_wheel_emote`; CB9C70 passes target and category to a
            // native service).
            "STU_3D419B63" if self.graph(id).index == 0x069a && matches!(node.pos, 7 | 8) => {
                let helpers: Vec<_> = self
                    .machine
                    .snapshot
                    .instances
                    .iter()
                    .filter(|(_, instance)| instance.graph == 0x1040)
                    .map(|(&helper, _)| helper)
                    .collect();
                for helper in helpers {
                    self.destroy(helper)?;
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
            "STU_1C2CC6D5" => self.clear_array(id, node, depth),
            "STU_4C3509FB" | "STU_CBE8BDAA" => self.insert_array(id, node, caller, depth),
            "STU_DA54F8F7" => self.remove_array(id, node, caller, depth),
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
                self.branch(id, node, caller, truthy(&value), depth)
            }
            "STU_821116A6" => {
                let Some(container) =
                    self.evaluate(id, node, field(&node.fields, "m_814F8EC3"), caller)
                else {
                    return Ok(false);
                };
                let Some(value) = self.evaluate(id, node, field(&node.fields, "m_value"), caller)
                else {
                    return Ok(false);
                };
                let contained = expr::contains(&container, &value);
                self.branch(id, node, caller, contained, depth)
            }
            "STU_7DEB522D" => {
                let Some(button) =
                    self.evaluate(id, node, field(&node.fields, "m_logicalButton"), None)
                else {
                    return Ok(false);
                };
                let button = expr::to_int(&button, 0);
                let held = self
                    .machine
                    .held_buttons
                    .iter()
                    .any(|&held| i64::from(held) == button);
                self.branch(id, node, caller, held, depth)
            }
            "STU_1113EBD5" => {
                // A join is only reached through a plug (`follow`).
                self.report(
                    self.site(id, node),
                    Issue::UnsupportedField("join input origin"),
                );
                Ok(false)
            }
            "STU_4E2BCAAB" => self.state_elapsed(id, node, caller, depth),
            "STU_BC5E4622" => self.cycle_index(id, node, depth),
            "STU_187D5125" => {
                self.stop_abilities(id, depth)?;
                self.follow(id, node, "m_outPlug", None, depth)?;
                Ok(true)
            }
            "STU_A807207A" => {
                if !self.dash(id, node)? {
                    return Ok(false);
                }
                self.follow(id, node, "m_outPlug", None, depth)?;
                Ok(true)
            }
            "STU_FF3471E9" => {
                if !self.velocity_action(id, node)? {
                    return Ok(false);
                }
                self.follow(id, node, "m_outPlug", None, depth)?;
                Ok(true)
            }
            "STU_0F647E55" => self.assign_team(id, node, depth),
            "STU_42578342" => {
                if !self.write_tag_query(id, node)? {
                    return Ok(false);
                }
                self.follow(id, node, "m_outPlug", None, depth)?;
                Ok(true)
            }
            "STU_D49BBD03" => self.set_parent_variable(id, node, depth),
            "STU_523EE38D" => self.destroy_entity(id, node, depth),
            "STU_FC3921B1" => self.create_dummy_bot(id, node, depth),
            "STUStatescriptActionCreateEntity" => self.create_entity(id, node, depth),
            "STU_C1561031" => self.remove_dummy_bot(id, node, depth),
            "STU_CFCE55DC" => self.move_body(id, node, depth),
            "STU_545AB3B8" => self.credit_environment(id, node, depth),
            "STU_FFF26F80" => self.set_ability_cooldown_of(id, node, depth),
            "STU_1E93FE1F" | "STU_DC325C52" | "STU_ADAFD7FF" | "STU_99ED4486" => {
                self.action_result(id, node, depth)
            }
            "STU_0BDBC7B2" => self.set_variable_of(id, node, depth),
            "STU_D137ECA0" => self.for_each_index(id, node, depth),
            "STU_A1DF3B2D" => self.related_entities(id, node, depth),
            "STU_08D6BA6E" => {
                self.cast_action(id, node)?;
                self.follow(id, node, "m_outPlug", None, depth)?;
                Ok(true)
            }
            // Restarts the `Anim` state whose handle `m_EDB2BD2A` holds
            // (`vm/timers.rs`). Its bool `m_A9220BE5` (Zenyatta's `0176`,
            // `02F3`) gates the overridden registration's execute
            // (`0x7FF789BCB670`), not the client's (`0x7FF78981C830`), which
            // the server follows. The run follows the out plug whatever the
            // execute did (`0x7FF78AA7EBA0`).
            "STU_6E18DEA0" => {
                let target = field(&node.fields, "m_EDB2BD2A");
                if !target.is_null()
                    && let Some(target) = self.evaluate(id, node, target, caller)
                {
                    self.restart_animation(&target)?;
                }
                self.follow(id, node, "m_outPlug", None, depth)?;
                Ok(true)
            }
            "STUStatescriptActionSwitch" => self.action_switch(id, node, depth),
            "STU_CEBDAF17" => {
                self.track_stat(id, node);
                self.follow(id, node, "m_outPlug", None, depth)?;
                Ok(true)
            }
            "STUStatescriptActionCriteria" => {
                self.raise_criteria(id, node);
                self.follow(id, node, "m_outPlug", None, depth)?;
                Ok(true)
            }
            class if SERVER_NO_OPS.contains(&class) => {
                if matches!(
                    class,
                    "STU_7106C356" | "STUStatescriptActionEffect" | "STU_100EF8C2"
                ) {
                    self.record_frame_action(id, node);
                }
                self.follow(id, node, "m_outPlug", None, depth)?;
                Ok(true)
            }
            "STU_621AF478" => self.revive_bodies(id, node, depth),
            "STU_AFBB59E9" => self.report_knockback(id, node, depth),
            _ => self.unhandled_action(id, node, depth),
        }
    }

    /// An action the VM has no model of: the world's no-op answer, a known
    /// no-op, or (tolerating unknowns) one that only follows its out plug;
    /// else refused.
    pub(super) fn unhandled_action(
        &mut self,
        id: u16,
        node: &Node,
        depth: usize,
    ) -> Result<bool, InitFault> {
        let class = node.class.as_str();
        let site = self.site(id, node);
        if let Some(answer) = self.world.no_op_action(class) {
            self.assume(site, class, answer);
        } else if ASSUMED_NO_OPS.contains(&class) {
            self.assume(site, class, "no-op");
        } else if self.tolerates_unknowns() {
            self.assume(site, class, "follows its out plug");
        } else {
            self.report(site, Issue::UnsupportedAction(node.class.clone()));
            return Ok(false);
        }
        self.follow(id, node, "m_outPlug", None, depth)?;
        Ok(true)
    }

    /// A criteria action changes nothing the VM tracks; it tells the world
    /// its identifier ([`World::criteria`]) for each entity it targets (the
    /// owner, or the entities `m_targets` evaluates to: the hero's `01CA`
    /// names his controller for its respawn and spawn lines): a literal, or
    /// an expression that gives one (`0686` node 56, the configurable wheel
    /// choices' `#29862.id(1384)`, the identifier its map gives the action),
    /// with its audience ([`RaisedCriteria::audience`]), read now: the
    /// graphs clear the list right after. An identifier or a target that
    /// can't be evaluated raises nothing: nothing here can fail.
    fn raise_criteria(&mut self, id: u16, node: &Node) {
        let targets = self.voice_targets(id, node);
        if targets.is_empty() {
            return;
        }
        let identifier = match node
            .field("m_identifier.m_identifier")
            .and_then(crate::graph::asset_guid)
        {
            Some(identifier) => Some(identifier),
            None => match self.evaluate_quietly(id, node, field(&node.fields, "m_identifier")) {
                Some(Value::Asset(identifier)) if identifier != 0 => Some(identifier),
                _ => None,
            },
        };
        if let Some(identifier) = identifier {
            let audience = self.criteria_audience(id, node);
            let site = self.site(id, node);
            for target in targets {
                self.world.criteria(
                    site,
                    RaisedCriteria {
                        identifier,
                        target,
                        audience: audience.clone(),
                    },
                );
            }
        }
    }

    /// The entities a criteria action or a voice stimulus state names in
    /// `m_targets`, each once ([`Self::entity_list`]).
    pub(super) fn voice_targets(&mut self, id: u16, node: &Node) -> Vec<u32> {
        self.entity_list(id, node, "m_targets")
    }

    /// The entities a node's field `name` names, each once: the owner
    /// without evaluating it, else what the expression gives (an entity or
    /// a list), none when it can't be evaluated.
    pub(super) fn entity_list(&mut self, id: u16, node: &Node, name: &str) -> Vec<u32> {
        let config = field(&node.fields, name);
        if config.get("$").and_then(Json::as_str) == Some("STU_B5A0CAF0") {
            return vec![self.site(id, node).entity];
        }
        if config.is_null() {
            return Vec::new();
        }
        let items = match self.evaluate_quietly(id, node, config) {
            Some(Value::Array(items)) => items,
            Some(entity @ Value::Entity(_)) => vec![entity],
            _ => return Vec::new(),
        };
        let mut entities = Vec::new();
        for item in items {
            if let Value::Entity(entity) = item {
                if expr::valid_entity(entity) && !entities.contains(&entity) {
                    entities.push(entity);
                }
            }
        }
        entities
    }

    /// A server-only `VoiceStimulus` state began: ask the world to say its
    /// stimulus ([`World::voice_stimulus`]). A stimulus, target or delay
    /// that can't be evaluated says nothing; the state stays on as a plain
    /// state either way, as the client's networked ones do.
    pub(super) fn begin_voice_stimulus(&mut self, key: StateKey, node: &Node) {
        let id = key.instance;
        let site = self.site(id, node);
        let stimulus = match self.evaluate_quietly(id, node, field(&node.fields, "m_stimulus")) {
            Some(Value::Asset(stimulus)) if stimulus != 0 => stimulus,
            _ => return,
        };
        let delay = match field(&node.fields, "m_B0263D2D") {
            Json::Null => 0.0,
            config => match self.evaluate_quietly(id, node, config) {
                Some(Value::Float(seconds)) => seconds,
                Some(Value::Integer(seconds)) => seconds as f64,
                _ => return,
            },
        };
        let targets = self.voice_targets(id, node);
        if targets.is_empty() {
            return;
        }
        let audience = self.criteria_audience(id, node);
        let request = VoiceRequest {
            stimulus,
            targets,
            delay,
            audience,
        };
        if !self.world.voice_stimulus(site, request) {
            self.assume(site, &node.class, "no line (the world plays none)");
        }
    }

    /// The entities a criteria action's `m_297E0B63` lists, in order, each
    /// once: none for no list, all players (`STU_BD2D717B`) or a list that
    /// can't be evaluated.
    fn criteria_audience(&mut self, id: u16, node: &Node) -> Option<Vec<u32>> {
        let config = field(&node.fields, "m_297E0B63");
        if config.is_null() || config.get("$").and_then(Json::as_str) == Some("STU_BD2D717B") {
            return None;
        }
        let entities = match self.evaluate_quietly(id, node, config)? {
            Value::Array(items) => items,
            Value::Entity(entity) => vec![Value::Entity(entity)],
            _ => return None,
        };
        let mut audience = Vec::new();
        for item in entities {
            if let Value::Entity(entity) = item {
                if expr::valid_entity(entity) && !audience.contains(&entity) {
                    audience.push(entity);
                }
            }
        }
        Some(audience)
    }

    /// SetVar (`indexed`) and one pair of SetVars. SetVar stores nothing when
    /// its target or its value is missing; a SetVars pair with a target and
    /// no value stores int 0 (`docs/VARIABLE_ASSIGNMENTS.txt` in
    /// leafall903/ORPHEUS, commit c239a84).
    pub(super) fn store(
        &mut self,
        id: u16,
        node: &Node,
        fields: &Json,
        indexed: bool,
    ) -> Result<bool, InitFault> {
        let target = field(fields, "m_out_Var");
        let value = field(fields, "m_value");
        if target.is_null() || (indexed && value.is_null()) {
            return Ok(true);
        }
        let value = if value.is_null() {
            Value::Integer(0)
        } else {
            let Some(value) = self.evaluate_value(id, node, value, None) else {
                return Ok(false);
            };
            value
        };
        let site = self.site(id, node);
        let target = match expr::lvalue(target) {
            Ok(Some(target)) => source_key(id, target),
            // `#v[index] = value` (`expr::element_lvalue`): the Workshop's
            // Set Variable At Index graphs.
            Ok(None) => {
                let Some(element) =
                    self.evaluate_with(id, node, |context| expr::element_lvalue(target, context))
                else {
                    return Ok(false);
                };
                return match element {
                    Some((variable, index)) => match usize::try_from(expr::to_int(&index, 0)) {
                        Ok(index) => {
                            self.write_element(site, source_key(id, variable), index, value)
                        }
                        Err(_) => Ok(true),
                    },
                    None => Ok(true),
                };
            }
            Err(error) => {
                self.report(site, Issue::Expression(error));
                return Ok(false);
            }
        };
        let key = field(fields, "m_key");
        if indexed && !key.is_null() {
            // A keyed insert: an entry with an equal key is replaced by the
            // new one at the end (`store` in nodes.py).
            let Some(key) = self.evaluate(id, node, key, None) else {
                return Ok(false);
            };
            let mut entries = match self.machine.value(target, None) {
                Value::Map(entries) => entries,
                _ => Vec::new(),
            };
            entries.retain(|(existing, _)| !equal(existing, &key));
            entries.push((key, value));
            self.write(target, Value::Map(entries))?;
            return Ok(true);
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
                return self.write_element(site, target, index, value);
            }
        }
        self.write(target, value)?;
        Ok(true)
    }

    /// Set item `index` of the array in `target`, which grows to hold it.
    fn write_element(
        &mut self,
        site: Site,
        target: VarKey,
        index: usize,
        value: Value,
    ) -> Result<bool, InitFault> {
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
            if matches!(kind, StateKind::Pulser { .. }) {
                self.pulse(key);
            }
            return Ok(());
        }
        let kind = match node.class.as_str() {
            // These client presentation states use the base state lifecycle on
            // the server, as every state class without a model does in the
            // Python reference (`ow174/game/script/nodes.py`).
            "STUStatescriptState"
            | "STU_9D7BF987"
            // ClientOnlySharedVars (`m_vars`): Small Message `1D7B` and
            // Create HUD Text `1D8A` share the rule's event context
            // (`#19467` Event Player, `#22213` Attacker, Victim…, read by
            // `1B4A`) with the clients; nothing for the server.
            | "STU_1B2CD7F2"
            | "STU_B4CD94F4"
            | "STU_90C3BCEA"
            | "STUStatescriptStateDataFlowMapping"
            | "STU_E13B30A8"
            | "STUStatescriptStateUXPresenter"
            | "STUStatescriptStateAnimatedCamera" => StateKind::Plain,
            // A UX presenter (parent `STUStatescriptStateUXPresenter`) whose
            // runtime vtable (0x7FF78B560B70) differs from its parent's only
            // in boilerplate and slot 58, 0x7FF7899E0240, which fills the
            // presenter's data model (`68B2.01C`, `65F3.01C`) from the local
            // player: presentation, as the base presenter's. Codec none.
            // `13C0` st0, `0CB6` st4, `0160` st12.
            "STU_62755D14" => StateKind::Plain,
            // The controller's `0C90` st82 on the mode's `0692` (setup),
            // `m_80243D28` = owner: its begin (0x7FF789C20DE0) adds one to a
            // count at component 74 `+0x100` of that entity and its end
            // (0x7FF789C21890) takes it away; the count is past the
            // component's mirror (not networked) and no client code reads it
            // (`who_reads.py --component 74 +0x100`, ControllerWorld.md).
            // Codec none. Nothing for the server to keep.
            "STU_D9498E7C" => StateKind::Plain,
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
            FOR_CODE_SYNCED if is_networked_for_code_synced(node) => StateKind::Plain,
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
            // The map entity states (`orpheus_script::placed`). Animation
            // alias request (`0D32`'s doors): while it is active the client
            // pushes the alias `m_11E0A658` on the entity's animation
            // component (leafall903/ORPHEUS `ANIMATION_ALIAS_STATE.txt`); the
            // server keeps the base lifecycle and sends the remote state. Its
            // true mode (`m_9507488A`) aborts at once and is not modelled.
            "STU_B1FA9C2A" if field(&node.fields, "m_9507488A").is_null() => StateKind::Plain,
            // Entity control (`00F6`'s barriers): its rules set flags on the
            // client's model of the entity (`ENTITY_CONTROL_STATE.txt`, the
            // same source); the server keeps the base lifecycle and sends the
            // remote state. Its payload is the link list (codec `link`).
            "STU_33A46033" => StateKind::Plain,
            "STU_7B9E8909" => StateKind::Plain,
            // A voice stimulus plays a line on the client that runs it (a
            // networked or client-only state: the server keeps it plain and
            // its frames carry it); a server-only one is the server's to say
            // ([`World::voice_stimulus`], on begin).
            "STU_40E05F6B" => StateKind::Plain,
            // The field of view (Ana's and Widowmaker's zoom, `0B94` st23,
            // `001B` st7) acts on the owner's camera, which the server
            // doesn't have: a plain state, as Python ran it, that its frames
            // carry for the client to run.
            "STU_75D1C308" => StateKind::Plain,
            // The scene tag query state, the spawn picker, the shuffled
            // picker and the placeable remover run the base lifecycle; their
            // work is `vm/entities.rs` (codec `none`).
            "STU_160F2578" | "STU_5E805B5A" | "STU_37072672" | "STU_A3DC6A25" => {
                StateKind::Plain
            }
            // The aim speed (`STU_3716E1D4`): its own begin, end and update
            // (0x7FF7897F6390, 0x7FF7897F63E0, 0x7FF7897F6410) only call
            // 0x7FF789808820, which scales the pose rates of the entity's
            // model (component 43; leafall903/ORPHEUS (MIT) `CorpseTimeScale`
            // in `nodes.py`). The server animates no model: a plain state its
            // frames carry (codec `link`, sent without a link). Soldier: 76's
            // death (`01C7` node 68), `0C8E` st7/st25, the scopes of Ana,
            // Ashe and Widowmaker.
            "STU_3716E1D4" => StateKind::Plain,
            // The mission state (`STU_25CBEFB0`, `1B4A` st0, empty
            // `m_108E51DD`): it shares `STU_9028E035`'s registered-state
            // hooks (begin/end 1B72530/1B725C0 add it to and remove it from
            // its class's list) and link codec (leafall903/ORPHEUS (MIT)
            // `RegisteredState` in `nodes.py`), without a status effect: a
            // plain state, sent like a status without a link.
            "STU_25CBEFB0" => StateKind::Plain,
            // The death layer `01CB`'s cameras (codec `none`, no update of
            // their own). STU_D75CDECC's own begin (0x7FF7898CF220) and end
            // (0x7FF7898CF840) post 014B/014C to the viewing client's HUD
            // graph `0250` when the local view is this body. CameraLookAt
            // (`STU_96333BBE`) and `STU_A103C85E` keep the base lifecycle
            // and act through extra slots on the entity's model (component
            // 43, 0x7FF789821920, 0x7FF78988F1C0). None ends by itself; the
            // server has no camera or HUD: plain states its frames carry.
            "STU_D75CDECC" | "STU_96333BBE" | "STU_A103C85E" => StateKind::Plain,
            "STUStatescriptStateHealthPool" if health_pool_form(node).is_some() => {
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
            "STUStatescriptStateWait" => StateKind::Wait,
            "STU_E3CEF6B9" => StateKind::ServerFrames,
            "STU_9C9A422A" => StateKind::Defer,
            "STU_B9898052" => match self.state(key).map(|state| state.kind) {
                // The pulse count carries over from earlier activations.
                Some(kind @ StateKind::Pulser { .. }) => kind,
                _ => StateKind::Pulser {
                    pulses: 0,
                    pulsed_ms: None,
                },
            },
            "STU_E4A30BCF" => StateKind::DisableButton { button: 0 },
            "STU_D7F9E1C3" => StateKind::AiFire { button: None },
            "STUStatescriptStateMoveTo" => StateKind::MoveTo,
            "STU_4BBC5D09" => StateKind::GrappleCast,
            "STU_1F5ED928" => StateKind::NavPath,
            // The scoped heroes' zoom-in input flag (Widowmaker `001B`
            // st103, Ana `0B94` st78, Ashe `14FC` st111; codec none). The
            // client's begin puts the state on its instance's update list
            // (`0x7FF78AA82DC0`); each frame its update (`0x7FF789808F70`)
            // writes `m_9A86AC74` (`#27203e`/`#27204e`/`#27205e`, set by no
            // graph, `&& #169 < 1.0`: the zoom is still coming in) as a byte
            // into component 43 `+0x270`, and its end clears it
            // (`0x7FF7897F64C0`). Only the client's own aim code reads that
            // byte (`0x7FF789671DA0`, with the mouse look scale at `+0x274`
            // that the input sampler applies); the server gets the commands
            // it shaped. So it is a plain state here: its activity goes out
            // in the owner frames, its value is the client's.
            "STU_D1BF5FA6" => StateKind::Plain,
            // A beam effect from a hardpoint to an entity variable's point
            // (Widowmaker's hook rope, `005A` st7/st53; codec none): the
            // client's presentation alone.
            "STU_9932D728" => StateKind::Plain,
            "STU_ECAEE401" => StateKind::AiMove,
            // The AI movement state's goals (`STU_B28C2E5E` move-to
            // `194D744E` and the unnamed `2C18F116`, `STU_627A461C` face-to
            // `AB9B30EF`): no registration of their own (the client runs the
            // base state); the `STU_ECAEE401` above them reads them
            // (`resolve_ai_move`). Data holders here.
            "STU_194D744E" | "STU_2C18F116" | "STU_AB9B30EF" => StateKind::Plain,
            "STU_41B3FC2B" => StateKind::Rewind {
                end_ms: 0,
                frames: 0,
            },
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
            "STUStatescriptStateRayCast" | "STUStatescriptStateTrackRay" => StateKind::Ray,
            "STU_FBFF5DE7" => StateKind::WallCheck,
            "STU_4C96470A" => StateKind::Landing,
            "STU_9935FBC8" => StateKind::ViewLimits { radians: [0; 6] },
            "STUStatescriptStateShockwave" => StateKind::Shockwave,
            "STU_0A6BE10E" if filter_array::is_runnable_filter_array(node) => {
                StateKind::FilterArray
            }
            "STU_0E913A0B" => StateKind::HeroSelect,
            // ForCode (the strict form of ForCodeSynced `STU_CD46AF93`): it
            // registers its rows (identifier, value) for native game code to
            // read and has no payload (codec "none"); its plugs and
            // lifecycle are the base state's (leafall903/ORPHEUS (MIT),
            // `docs/FOR_CODE_BINDINGS.txt`: D44CA0 registers, D0D8B0 reads
            // the first match, D44CD0 removes). On the server the native
            // readers are the match's Rust code, which reads the graph's
            // variables itself; so it runs as a plain state. The forms with
            // transport sync (`m_598E32D5`, `m_0582B0E4`) are not known.
            FOR_CODE
                if field(&node.fields, "m_598E32D5").as_u64().unwrap_or(0) == 0
                    && field(&node.fields, "m_0582B0E4").as_u64().unwrap_or(0) == 0 =>
            {
                StateKind::Plain
            }
            "STUStatescriptStateModifyHealth" => StateKind::ModifyHealth { last_ms: 0 },
            "STU_9028E035" => StateKind::Status,
            "STUStatescriptStateCombatModFilter" => StateKind::CombatMod,
            "STU_431F3FCC" => StateKind::ApplyAura,
            // The assist credit (`vm/statuses.rs`): Biotic Grenade's
            // auras (`0C72` st7, `0B99` st7), Discord and Harmony Orbs, the
            // Biotic Field's heal and 26 more.
            "STU_0DB9E66B" => StateKind::Assist,
            // Biotic Grenade's auras (`0C72` st9, `0B99` the same): it
            // takes the aura's source (`m_source`, `STU_DFAB9029` of #18763
            // and the instigator) into #18779, which only the aura's health
            // change reads (`m_A3A0BDD6`, unread). It changes nothing the
            // server tracks; assumed plain.
            class @ "STU_85124386" => {
                self.assume(self.site(id, node), class, "plain state");
                StateKind::Plain
            }
            // The Biotic Orbs' `10FF` rid10 and `1100` rid8, while the orb
            // has targets: the client's begin sets its component 50
            // `+0x3AC` to `m_67E8FF48` (0.275) and its end back to 1
            // (`0x7FF789C6C7D0`, `0x7FF789C6CAA0`; BioticOrb.md §2, §4, §6),
            // a replicated field no client code was found to read: the
            // projectile's speed scale, as an assumption (the orb slows
            // while it tethers a body). The world flies the projectile at
            // that scale (`World::set_projectile_speed`).
            "STU_D3F71FCF" => StateKind::ProjectileSpeed,
            // TrackStatTime: the seconds it runs go to its stats
            // (`vm/stats.rs`).
            "STU_441B50E4" => StateKind::StatTime,
            // Server-only states (no registration of their own: the client
            // runs the base state, codec `none`), with a meaning the data
            // doesn't tell: `STU_F08E1453` names the owner (`m_64C3E07E`) in
            // 23 graphs (Tracer's Recall `001A` st26, Zarya's `0256` st10,
            // the body's `01CB` st35), beside an entity control
            // (`STU_33A46033`); what it does on the server is unknown. A
            // plain state, as an assumption.
            "STU_F08E1453" => {
                self.assume(
                    self.site(id, node),
                    &node.class,
                    "plain state (server-only, meaning unknown)",
                );
                StateKind::Plain
            }
            // The pickup pair (server-only, `vm/pickups.rs`): the health
            // pack's pickup `0046` st5 and the hurt body's search `0033`
            // st11 for the pickup it touches.
            "STU_913C09E4" => StateKind::Pickup,
            "STU_87621906" => StateKind::PickupSearch { found: None },
            "STUStatescriptStateCreateEntity" => StateKind::CreateEntity { entity: None },
            // A wait the server ends (`begin_server_wait`, `end_wind_up`).
            "STU_54FEB21C" => StateKind::Plain,
            class if ASSUMED_PLAIN_STATES.contains(&class) => StateKind::Plain,
            class if ASSUMED_INERT_STATES.contains(&class) => StateKind::Plain,
            "STU_77E13026" => StateKind::HitList,
            "STU_550D0C5C" => StateKind::Scope,
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
            StateKind::ServerFrames => self.arm_server_frames(key, node)?,
            StateKind::Defer => self.begin_defer(key, node)?,
            StateKind::Pulser { .. } => self.pulse(key),
            StateKind::DisableButton { .. } => self.begin_disable_button(key, node, depth)?,
            StateKind::AiFire { .. } => self.begin_ai_fire(key, node)?,
            StateKind::MoveTo => self.begin_move_to(key, node),
            StateKind::GrappleCast => self.begin_grapple(key, node, depth)?,
            StateKind::NavPath => self.begin_nav_path(key, node)?,
            StateKind::AiMove => self.resolve_ai_move(key, node),
            StateKind::Rewind { .. } => self.begin_rewind(key, node)?,
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
                self.update_movement_sample(key, node, previous, false)?;
                self.register_tick(key, self.machine.now_ms);
            }
            StateKind::Anim => self.arm_animation(key, node, depth)?,
            StateKind::SendMessage => self.begin_send_state(key, node)?,
            StateKind::CosmeticEntity => self.begin_cosmetic(key, node, depth)?,
            StateKind::ChaseVar => self.begin_chase(key, node, depth)?,
            StateKind::Watch => {
                let seen = self.evaluate(id, node, field(&node.fields, "m_B7A3CF78"), Some(key));
                self.state_mut(key).expect("active Watch").watch_seen = seen;
            }
            StateKind::FilterArray => self.refresh_filter_array(key)?,
            StateKind::HeroSelect => self.refresh_hero_select(key),
            StateKind::WeaponVolley { .. } => self.begin_volley(key, node, depth)?,
            StateKind::TrackTargets => self.begin_track_targets(key, node)?,
            StateKind::Ray => self.begin_ray(key, node, depth)?,
            StateKind::WallCheck => self.begin_wall_check(key, node)?,
            StateKind::Landing => self.begin_landing(key, node)?,
            StateKind::ViewLimits { .. } => self.refresh_view_limits(key),
            StateKind::Shockwave => self.begin_shockwave(key, node)?,
            StateKind::ModifyHealth { .. } => self.begin_modify_health(key, node)?,
            StateKind::Status => self.begin_status(key, node),
            StateKind::ProjectileSpeed => self.begin_projectile_speed(key, node),
            StateKind::CombatMod => self.begin_combat_mod(key, node),
            StateKind::Assist => self.begin_assist(key, node),
            StateKind::ApplyAura => {
                self.assume(
                    self.site(id, node),
                    "STU_431F3FCC lifetime",
                    "on each listed body while listed (server-only rule)",
                );
                self.refresh_auras(key, node)?;
                self.register_tick(key, self.machine.now_ms);
            }
            StateKind::HitList => self.assume(self.site(id, node), &node.class, "no hits"),
            StateKind::Scope => self.begin_scope(key, node, depth)?,
            StateKind::CreateEntity { .. } => self.begin_create_entity(key, node)?,
            StateKind::Pickup => self.begin_pickup(key, node),
            StateKind::PickupSearch { .. } => {
                self.search_pickup(key, node, true)?;
                self.register_tick(key, self.machine.now_ms);
            }
            StateKind::Plain if node.class == "STU_160F2578" => {
                if !self.write_tag_query(id, node)? {
                    return Ok(());
                }
            }
            StateKind::Plain if node.class == "STU_37072672" => {
                if !self.pick_shuffled(id, node)? {
                    return Ok(());
                }
            }
            StateKind::Plain if node.class == "STU_A3DC6A25" => {
                if !self.remove_placeables(id, node)? {
                    return Ok(());
                }
            }
            // The client's own run finishes the state once its line has
            // played (`0x7FF789C54090` keeps the line's end at `+488` and
            // then calls `0x7FF78AA812E0`); a state left on would ignore the
            // next begin: `01CA` st46 says one line per kill. The server
            // finishes it at once (it doesn't know the line's length; `01CA`
            // st34's finished plug only clears a list).
            StateKind::Plain if node.class == "STU_40E05F6B" && node.server_only => {
                self.begin_voice_stimulus(key, node);
                self.post(id, Event::Finish(key.raw))?;
            }
            StateKind::Plain if node.class == "STU_54FEB21C" && is_server_ended(node) => {
                self.end_wind_up(key, node)?;
            }
            StateKind::Plain if node.class == "STU_54FEB21C" => {
                self.begin_server_wait(key, node)?;
            }
            StateKind::Plain if ASSUMED_PLAIN_STATES.contains(&node.class.as_str()) => {
                self.assume(self.site(id, node), &node.class, "plain state");
            }
            StateKind::Plain if super::for_code_transport(node) => {
                self.assume(self.site(id, node), &node.class, "rows without transport");
            }
            StateKind::Plain if ASSUMED_INERT_STATES.contains(&node.class.as_str()) => {
                self.assume(self.site(id, node), &node.class, "no effect");
            }
            StateKind::Plain
            | StateKind::MovementMod
            | StateKind::Effect
            | StateKind::ActiveQuery
            | StateKind::StatTime => {}
        }
        self.follow(id, node, "m_onBeginPlug", Some(key), depth)?;
        self.follow(id, node, "m_subgraphPlug", Some(key), depth)?;
        self.try_complete(key, node, depth)
    }
}
