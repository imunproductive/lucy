//! The communication wheel's PlayScript actions and the networked actions
//! its graphs send the clients.
//!
//! Ported from leafall903/ORPHEUS (MIT): `play.resolve_request`,
//! `play_queue.Queue` and `Instance.start_entries` (906E49..9070D0,
//! 64D2C0/64CD10, 1B95950), `BodyWorld.play_script` and
//! `BodyWorld.replicate_wheel_event` in `driver.py`. The wheel's own
//! self-targeted PlayScript nodes ([`wheel::PLAY_SCRIPTS`]), the aura
//! graphs' (`vm/auras.rs`), the hero body's attacker list
//! ([`ATTACKER_LIST`]), the Workshop's actions' (Chase Variable `1CCC`
//! playing `1CCF` on a player, [`World::play_script`]) and a hero
//! ability's plays on the other bodies it struck ([`Plays::Others`]) run;
//! every other PlayScript stays a no-op, as in the Python reference.
//!
//! A request needs a target that is the body itself, or a world that
//! plays it on the other entity. Its parameters are
//! always arrays (1BA7C60). Policy (`m_AA4DE426`) 0 keeps a running instance
//! of the graph and does nothing, 2 reuses it, 3 replaces it; otherwise a
//! new root instance is made. Then each start entry (`STU_BB4AC733`) takes
//! the parameters named in its rows and the caller in `m_B60ABDA4`, and runs.

use super::*;

/// The hero body's `01CF` rid175 (node 201, server-only): on the damage
/// message `0011.025` it plays `1901` on the body for an attacker not yet
/// listed, with the attacker's owner (`STU_F76C486C`, #9737) as `06C5`.
/// `1901` (7 nodes, all server-only) adds him to the body's #17557e, and
/// takes him off 5 s after his last hit (each `0011.025` from him restarts
/// the wait). No graph reads the list but `01CF` itself (a listed attacker
/// plays no second `1901`) and the client's nameplate `0D24` st55: the
/// retail server's credit for assists was its own.
pub const ATTACKER_LIST: ((u32, u16), u32) = ((0x01cf, 201), 0x1901);

/// A health pack's script, `0216` (big: 250 health, 15 s) or `0217`
/// (small: 75, 10 s), the server placeables `0FC2`/`0FC3` start: its one
/// PlayScript plays `0046` on the pack itself, which heals and respawns
/// it, and one server frame after it starts (rid28 WaitForServerFrames 1)
/// reads the ruleset's "Spawn Health Packs" (`#9542`, rid29): Disabled,
/// or Determined By Mode with the mode's `#9664` set, removes the pack
/// (rid31 `STU_523EE38D` on `owner()`). `0046` itself plays `0EB9` (rid37)
/// on the visual its StateCreateEntity st15 made (#38) while the pack is
/// up (`vm/pickups.rs`).
pub const HEALTH_PACKS: [u32; 3] = [0x0216, 0x0217, HEALTH_PACK];

/// The health pack graph `0046`: its ModifyHealth st0 is the pack's heal
/// (the Workshop's Event Was Health Pack, `lucy_sim::combat`).
pub const HEALTH_PACK: u32 = 0x0046;

/// Where a PlayScript action plays its graph ([`Vm::play_script`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Plays {
    /// On each of its targets, the entity itself included.
    Everywhere,
    /// On its targets other than the entity itself: a hero ability's
    /// victim graph (Rocket Punch `0D06` plays `0D07` on #375, Soundwave
    /// `0776` plays `0ECA` on #375), which the world plays in the struck
    /// body's script ([`World::play_script`]). Its plays on its own body
    /// stay no-ops, as before.
    Others,
}

/// Doomfist's passive The Best Defense `0DDD`: on each damage his abilities
/// deal (`0012.025`, tags `245A` 30 shields, `01FD` 75) its rid4 plays
/// `0DF3` on him with the grant (`002A`, `min` of the room under the 150
/// cap and the grant) and the cap (`03D1`); `0DF3` st0 is the grant's
/// temporary shield pool, named `249D.01C`, and exits when it runs out.
/// `0DDD` st1 drains the named pools 3 a second, 1 s after the last grant.
pub const BEST_DEFENSE: u32 = 0x0ddd;

/// The graph a server PlayScript node (not the wheel's) plays, if it is one.
pub(super) fn server_played_graph(graph: u32, node: u16) -> Option<u32> {
    let (site, played) = ATTACKER_LIST;
    (site == (graph, node)).then_some(played)
}

impl<W: World + ?Sized> Vm<'_, '_, '_, W> {
    pub(super) fn play_script(
        &mut self,
        id: u16,
        node: &Node,
        graph: u32,
        plays: Plays,
    ) -> Result<(), InitFault> {
        let resource = node.field("m_script.m_graph").and_then(asset_guid);
        if resource != Some(0x0580 << 48 | u64::from(graph)) {
            self.report(
                self.site(id, node),
                Issue::UnsupportedField("PlayScript resource"),
            );
            return Ok(());
        }
        let entity = self.machine.snapshot.entity;
        let Some(targets) = self.evaluate(id, node, field(&node.fields, "m_targets"), None) else {
            return Ok(());
        };
        let mut targets: Vec<u32> = match targets {
            Value::Entity(target) => vec![target],
            Value::Array(items) => items
                .iter()
                .filter_map(|item| match item {
                    Value::Entity(target) => Some(*target),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        if plays == Plays::Others {
            targets.retain(|&target| target != entity);
        }
        if targets.is_empty() {
            return Ok(());
        }
        let Some(parameters) = self.play_parameters(id, node) else {
            return Ok(());
        };
        let policy = node
            .fields
            .get("m_AA4DE426")
            .and_then(Json::as_i64)
            .unwrap_or(0);
        let site = self.site(id, node);
        for target in targets {
            if target == entity {
                self.play_here(graph, &parameters, policy, self.source_of(id), entity)?;
                continue;
            }
            // Another entity's: a world that plays none there leaves it.
            let request = PlayRequest {
                target,
                graph,
                parameters: parameters.clone(),
                policy,
                caller: entity,
                site,
            };
            self.world.play_script(site, request);
        }
        Ok(())
    }

    /// Play `graph` on this entity with its start entries' `parameters`:
    /// `policy` 0 keeps a running instance, 2 restarts its entries, 3
    /// replaces it, others add another.
    pub(super) fn play_here(
        &mut self,
        graph: u32,
        parameters: &[(u64, Value)],
        policy: i64,
        source: u32,
        caller: u32,
    ) -> Result<(), InitFault> {
        let running: Vec<u16> = self
            .machine
            .snapshot
            .instances
            .iter()
            .filter(|(_, instance)| instance.graph == graph)
            .map(|(&running, _)| running)
            .collect();
        let target = match (policy, running.first()) {
            (0, Some(_)) => return Ok(()),
            (2, Some(&running)) => running,
            _ => {
                if policy == 3 {
                    for running in running {
                        self.destroy(running)?;
                    }
                }
                let created = u16::try_from(self.machine.next_id)
                    .map_err(|_| InitFault::InstanceIdExhausted)?;
                if !self.create(created, graph, None, Variables::new(), source)? {
                    return Ok(());
                }
                created
            }
        };
        self.start_entries(target, parameters, caller)
    }

    /// A PlayScript's (or `STU_CB4905E9`'s) `m_params` rows: each key with
    /// its value as an array (1BA7C60). None: a value can't be evaluated.
    pub(super) fn play_parameters(&mut self, id: u16, node: &Node) -> Option<Vec<(u64, Value)>> {
        let mut parameters = Vec::new();
        for row in array_field(&node.fields, "m_params") {
            let Some(key) = row.get("m_B5051BCE").and_then(asset_guid) else {
                continue;
            };
            let value = match field(row, "m_value") {
                Json::Null => Value::Array(Vec::new()),
                config => match self.evaluate_value(id, node, config, None)? {
                    Value::Array(items) => Value::Array(items),
                    Value::Null => Value::Array(Vec::new()),
                    value => Value::Array(vec![value]),
                },
            };
            parameters.push((key, value));
        }
        Some(parameters)
    }

    /// `Instance.start_entries`: bind each start entry's named parameters,
    /// then queue it. A parameter of one item binds as the item, as the
    /// client's binder does (`sub_7FF78AAA5950` copies each parameter and
    /// `sub_7FF789E78020` replaces a plain array of one by its item): the
    /// Workshop's Chase Variable `1CCF` reads its rate and destination
    /// (`#2316`, `#13151`) with `STU_77B0987E`, which takes only a value
    /// tree, not an array holding one.
    pub(super) fn start_entries(
        &mut self,
        id: u16,
        parameters: &[(u64, Value)],
        caller: u32,
    ) -> Result<(), InitFault> {
        let graph = self.graph(id);
        for &position in graph.entries.iter().flatten() {
            let Some(entry) = graph.node(position) else {
                continue;
            };
            if entry.client_only || entry.class != "STU_BB4AC733" {
                continue;
            }
            let disabled = field(&entry.fields, "m_5AEA0A51");
            if disabled.as_bool() == Some(true) || disabled.as_i64().is_some_and(|flag| flag != 0) {
                continue;
            }
            for (key, value) in parameters {
                let row = array_field(&entry.fields, "m_params").iter().find(|row| {
                    row.get("m_B5051BCE").and_then(asset_guid) == Some(*key)
                        && !field(row, "m_out_Var").is_null()
                });
                if let Some(row) = row {
                    let value = match value {
                        Value::Array(items) if items.len() == 1 => items[0].clone(),
                        value => value.clone(),
                    };
                    self.bind_start_value(id, entry, field(row, "m_out_Var"), value)?;
                }
            }
            let source = field(&entry.fields, "m_B60ABDA4");
            if !source.is_null() && expr::valid_entity(caller) {
                self.bind_start_value(id, entry, source, Value::Entity(caller))?;
            }
            self.post(id, Event::Entry(position))?;
        }
        Ok(())
    }

    pub(super) fn bind_start_value(
        &mut self,
        id: u16,
        entry: &Node,
        target: &Json,
        value: Value,
    ) -> Result<(), InitFault> {
        match expr::lvalue(target) {
            Ok(Some(target)) => self.write(source_key(id, target), value),
            Ok(None) => Ok(()),
            Err(error) => {
                self.report(self.site(id, entry), Issue::Expression(error));
                Ok(())
            }
        }
    }

    /// A networked action with a remote index ran: keep it for the
    /// clients' frames, with the values its remote sync list
    /// (`m_BF5B22B7`, `STUStatescriptRemoteSyncVar`: the variable
    /// `m_0D09D2D9`, its scope `m_56341592`, 0 the instance's, 1 the
    /// entity's) has as it runs. A remote index (`m_remoteSyncNodes`) is
    /// how the server tells the clients that the node ran: the client
    /// runs the node it names from the frame's remote messages (104319
    /// A0CCA0), and the owner's client records the actions it predicted
    /// itself (the base run, 0x7FF78AA7EBA0, host vt+312 with the node's
    /// uid). Any entity's graphs: the mode entity's (`0F4A`'s music and
    /// countdown messages), a body's (`01C7`'s hit and elimination
    /// effects), the wheel's, a Workshop action's (Small and Big
    /// Message's `STU_100EF8C2`).
    pub(super) fn record_frame_action(&mut self, id: u16, node: &Node) {
        if node.remote < 0 || !node.is_action() || !node.is_networked() {
            return;
        }
        let values = remote_sync_variables(node)
            .map(|variable| {
                let key = source_key(id, variable);
                let owner = match variable.scope {
                    Scope::Instance => id,
                    Scope::Entity => 0,
                };
                (owner, variable.id, self.machine.value(key, None))
            })
            .collect();
        let instance = &self.machine.snapshot.instances[&id];
        let instance_start_ms = instance.start_ms;
        let owner = self.machine.in_message || wheel::GRAPHS.contains(&instance.graph);
        let actions = &mut self.machine.snapshot.frame_actions;
        let serial = actions.last().map_or(1, |last| last.serial.wrapping_add(1));
        actions.push(FrameAction {
            serial,
            instance: id,
            instance_start_ms,
            node: node.pos,
            time_ms: self.machine.now_ms,
            values: Arc::new(values),
            owner,
        });
        if actions.len() > MAX_FRAME_ACTIONS {
            actions.remove(0);
        }
    }
}

/// The variables a networked action's remote sync list names
/// ([`Vm::record_frame_action`]); a row of another shape names none.
fn remote_sync_variables(node: &Node) -> impl Iterator<Item = Variable> + '_ {
    array_field(&node.fields, "m_BF5B22B7")
        .iter()
        .filter(|row| field(row, "$").as_str() == Some("STUStatescriptRemoteSyncVar"))
        .filter_map(|row| {
            let id = asset_guid(field(row, "m_0D09D2D9"))
                .filter(|guid| guid >> 48 == 0x0d80 && *guid as u16 != 0)?
                as u16;
            let scope = match field(row, "m_56341592").as_u64()? {
                0 => Scope::Instance,
                1 => Scope::Entity,
                _ => return None,
            };
            Some(Variable {
                scope,
                id: u32::from(id),
            })
        })
}
