//! Prepared owner statescript for a live body in build 104319.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io,
    sync::Arc,
    time::Duration,
};

use serde_json::Value as Json;

use crate::{
    combat::{BodyCombat, MELEE_GRAPH, PosedHitscan, PosedMelee, PosedProjectile, PosedShot},
    hitscan::aim_direction,
    instance::RETAIL_QUANTA,
    others::ObserverActivity,
    pools::{BODY_GRAPH, FALLBACK_POOLS, HealthPools},
    projectiles::{
        ProjectileLeases, local_prediction, predicted_sync, projectiles_per_shot,
        spawned_definitions,
    },
    replication::{BodyStep, OwnerBodyRuntime},
    voice::{SpawnBark, begin_spawn_bark},
    weapon_spread::{ring_directions, shot_direction},
};
use orpheus_content::heroes::Hero;
#[cfg(any(test, feature = "fixtures"))]
use orpheus_content::{heroes::SOLDIER, maps::PRACTICE_RANGE_MODE};
use orpheus_core::guid::parse_guid;
use orpheus_physics::{
    mover::Mover,
    mover::modifiers::{Modifier, ModifierKey},
};
use orpheus_script::{
    codecs::CodecTable,
    expr::to_float,
    graph::GraphStore,
    player::PlayerWorld,
    snapshot::{SnapshotEncoder, View},
    timeline::FRAME_MS,
    ultimate::{ChargeRates, UltimateCharge},
    vitals::Vitals,
    vm::{
        Component, GameMessage, MAX_INIT_WORK, Motion, MoveRequest, Root, Snapshot, StateKind,
        TargetCandidate, Unknowns, World,
    },
    weapons::{HitDefinition, ProjectileBallistics, pellet_rings},
    wire::{Value, Variables},
};
use orpheus_wire::{
    commands::Command,
    entity::{Component as EntityComponent, Movement},
};

/// The owner's health for the graphs, and the HUD panel's current and
/// maximum health: entity variables no graph writes (driver.py
/// `OWNER_HEALTH`, `HUD_HEALTH`, `HUD_MAX_HEALTH`).
const OWNER_HEALTH: u32 = 253;
const HUD_HEALTH: u32 = 1732;
const HUD_MAX_HEALTH: u32 = 1737;

#[derive(Clone, Copy)]
pub struct OwnerBodySpawn {
    pub controller: u32,
    pub entity: u32,
    pub body_filter: u64,
    pub slot: u8,
    pub frame: u32,
    pub pose: Movement,
    pub quanta: u32,
}

pub struct PlayerBodyFactory {
    graphs: Arc<GraphStore>,
    codecs: Arc<CodecTable>,
    mode: u64,
    hero: u64,
    roots: Vec<Root>,
    variables: Variables,
    pools: Option<HealthPools>,
    target_tags: Option<BTreeSet<u64>>,
    controller_on_fire: bool,
    combat: BodyCombat,
    unknowns: Unknowns,
    /// How fast the hero's ultimate charges.
    ult_rates: ChargeRates,
}

impl PlayerBodyFactory {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare(
        graphs: Arc<GraphStore>,
        codecs: Arc<CodecTable>,
        mode: u64,
        hero: &Hero,
        pools: Option<HealthPools>,
        controller_on_fire: bool,
        combat: BodyCombat,
        unknowns: Unknowns,
        ult_rates: Option<ChargeRates>,
        probe: OwnerBodySpawn,
    ) -> io::Result<Self> {
        let body = graphs
            .bodies()
            .get(&hero.body)
            .filter(|body| body.hero.as_deref().and_then(parse_guid) == Some(hero.guid))
            .ok_or_else(|| {
                unsupported("the hero body definition is absent from the graph bundle")
            })?;
        let roots = orpheus_script::vm::player_roots(body, &graphs).map_err(unsupported)?;
        let health = pools.unwrap_or(FALLBACK_POOLS).total();
        let variables = orpheus_script::vm::player_entity_variables(body, health, health);
        let target_tags = body.target_tags.clone();
        let ult_rates =
            ult_rates.unwrap_or_else(|| ChargeRates::of(graphs.rulesets(), hero.guid, mode));
        let factory = Self {
            graphs,
            codecs,
            mode,
            hero: hero.guid,
            roots,
            variables,
            pools,
            target_tags,
            controller_on_fire,
            combat,
            unknowns,
            ult_rates,
        };
        let runtime = factory.start(probe)?;
        let encoder = runtime.encoder();
        let snapshot = runtime
            .snapshot()
            .expect("successful body startup has a snapshot");
        let projected = encoder
            .project(snapshot, View::Owner)
            .map_err(unsupported)?;
        encoder.full(&projected).map_err(unsupported)?;
        Ok(factory)
    }

    pub fn start(&self, spawn: OwnerBodySpawn) -> io::Result<PlayerBodyRuntime> {
        if spawn.quanta != RETAIL_QUANTA {
            return Err(unsupported("the owner VM requires the 16 ms retail clock"));
        }
        let has_authored_pools = self.roots.iter().any(|root| root.graph == BODY_GRAPH);
        let mut world = PlayerWorld::new(
            Arc::clone(&self.graphs),
            spawn.controller,
            spawn.entity,
            spawn.body_filter,
            self.mode,
            self.hero,
        );
        world.position = spawn.pose.position;
        world.movement = spawn.pose;
        if !has_authored_pools {
            world.vitals = self.pools.unwrap_or(FALLBACK_POOLS).vitals();
        }
        world.pool_types_known = self.pools.is_some() || has_authored_pools;
        world.target_tags = self.target_tags.clone();
        world.controller_on_fire = self.controller_on_fire;
        world.unknowns = self.unknowns;
        let mut component = Component::start(
            Arc::clone(&self.graphs),
            spawn.entity,
            self.variables.clone(),
            self.roots.clone(),
            spawn.frame,
            &mut world,
            MAX_INIT_WORK,
        )
        .map_err(unsupported)?;
        component
            .advance_frame(spawn.frame, &mut world)
            .map_err(unsupported)?;
        world.vitals.clear_changed();
        let modifiers = component
            .movement_modifiers(&mut world)
            .map_err(unsupported)?;
        let leases = ProjectileLeases::from_authored(
            &self.graphs,
            component.snapshot().expect("body startup has a snapshot"),
            spawn.slot,
            spawn.frame,
            |instance, raw, config| {
                component.evaluate_state_config(instance, raw, config, &mut world)
            },
        )?;
        // driver.py `BodyScript._spawn`: the abilities that start on the
        // ultimate's charge.
        let ultimate = UltimateCharge::new(&component, &self.graphs, self.ult_rates);
        let runtime = PlayerBodyRuntime {
            component,
            world,
            codecs: Arc::clone(&self.codecs),
            modifiers,
            leases,
            frame: spawn.frame,
            ultimate,
            melee_seen: HashMap::new(),
            combat: self.combat.clone(),
            bark: None,
        };
        runtime.log_assumptions();
        Ok(runtime)
    }
}

/// Every assumption once per server run, as the Python reference warned
/// once: the answers the server gives without knowing they are right,
/// including, in release builds, the graph content it tolerates.
fn log_assumption(graph: u32, node: Option<u16>, class: &str, answer: &str) {
    use std::{
        collections::HashSet,
        sync::{LazyLock, Mutex},
    };
    /// Graph, node and class of each assumption already logged.
    type Logged = HashSet<(u32, Option<u16>, String)>;
    static LOGGED: LazyLock<Mutex<Logged>> = LazyLock::new(Mutex::default);
    let mut logged = LOGGED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if logged.insert((graph, node, class.to_owned())) {
        eprintln!("statescript graph {graph:04X} node {node:?}: {class} -> {answer}");
    }
}

pub struct PlayerBodyRuntime {
    component: Component,
    world: PlayerWorld,
    codecs: Arc<CodecTable>,
    modifiers: Option<BTreeMap<ModifierKey, Modifier>>,
    leases: ProjectileLeases,
    frame: u32,
    ultimate: UltimateCharge,
    /// The activation of each quick melee state seen last (combat.py
    /// `melee_seen`).
    melee_seen: HashMap<(u16, u16), u16>,
    /// The body's eyes, quick melee's states and the Biotic Field.
    combat: BodyCombat,
    /// The spawn voice line while it plays (voicelines.py).
    bark: Option<SpawnBark>,
}

impl PlayerBodyRuntime {
    fn log_assumptions(&self) {
        for item in self
            .component
            .snapshot()
            .into_iter()
            .flat_map(|snapshot| &snapshot.assumptions)
        {
            log_assumption(item.site.graph, item.site.node, &item.class, item.answer);
        }
    }

    /// combat.py `_melee`: quick melee's ModifyHealth state (0043 st4) hits
    /// what its TrackTargets would find, once per activation: a sphere ahead
    /// of the eye along the aim. The VM can't track targets, so the match
    /// finds them. Each activation is seen whether or not a pose came with
    /// it.
    fn melee(&mut self, command: Option<Command>, mover: &Mover) -> Vec<PosedShot> {
        let Some(snapshot) = self.component.snapshot() else {
            return Vec::new();
        };
        let mut started = Vec::new();
        for (&id, instance) in &snapshot.instances {
            for melee in self.combat.melee.iter() {
                let (graph, raw, reach, radius) =
                    (MELEE_GRAPH, melee.state, melee.reach, melee.radius);
                if instance.graph != graph {
                    continue;
                }
                let Some(state) = instance.states.get(&raw) else {
                    continue;
                };
                let seen = self
                    .melee_seen
                    .insert((id, raw), state.activation)
                    .unwrap_or(0);
                if state.activation != seen && state.active {
                    started.push((id, raw, graph, reach, radius));
                }
            }
        }
        let Some(command) = command else {
            return Vec::new();
        };
        let [x, y, z] = mover.pose().position;
        let eye = [x, y + self.combat.eyes.height(mover.crouched()), z];
        let aim = aim_direction(&command);
        let graphs = Arc::clone(&self.world.graphs);
        started
            .into_iter()
            .filter_map(|(id, raw, graph, reach, radius)| {
                let node = graphs.graph(graph)?.state(usize::from(raw))?;
                let modify = node
                    .field("m_modifyHealth")
                    .filter(|value| !value.is_null())?;
                let mut evaluate = |field: &str| {
                    modify
                        .get(field)
                        .filter(|value| !value.is_null())
                        .and_then(|config| {
                            self.component
                                .evaluate_state_config(id, raw, config, &mut self.world)
                                .ok()
                        })
                };
                let amount = evaluate("m_amount").map_or(0.0, |value| to_float(&value));
                let loadout = match evaluate("m_loadout") {
                    Some(Value::Asset(guid)) => Some(guid),
                    _ => None,
                };
                Some(PosedShot::Melee(PosedMelee {
                    source: self.world.entity,
                    centre: std::array::from_fn(|axis| eye[axis] + aim[axis] * reach),
                    radius,
                    amount,
                    loadout,
                    command,
                }))
            })
            .collect()
    }
}

impl OwnerBodyRuntime for PlayerBodyRuntime {
    fn vitals(&self) -> Option<&Vitals> {
        Some(&self.world.vitals)
    }

    fn take_health_update(&mut self) -> Option<EntityComponent> {
        let vitals = &mut self.world.vitals;
        if !vitals.has_changed() {
            return None;
        }
        vitals.clear_changed();
        Some(vitals.cumulative_component())
    }

    fn snapshot(&self) -> Option<&Snapshot> {
        self.component.snapshot()
    }

    fn encoder(&self) -> SnapshotEncoder<'_> {
        SnapshotEncoder::new(&self.world.graphs, &self.codecs).with_unknowns(self.world.unknowns())
    }

    fn advance(&mut self, command: Command, mover: &Mover, step: BodyStep) -> io::Result<()> {
        if command.frame <= self.frame
            || (step != BodyStep::UnposedTimer && command.frame != self.frame.saturating_add(1))
        {
            return Err(unsupported(
                "the owner VM frame is outside the command sequence",
            ));
        }
        let pose = mover.pose();
        self.world.position = pose.position;
        // Its own body, as the target searches see it, stands where it moved.
        let entity = self.world.entity;
        if let Some(own) = self
            .world
            .targets
            .iter_mut()
            .flatten()
            .find(|candidate| candidate.entity == entity)
        {
            own.feet = pose.position;
        }
        // Command gaps and timer advances retain the last movement record.
        if step == BodyStep::Posed {
            self.world.movement = pose;
        }
        if step == BodyStep::UnposedTimer {
            self.component
                .advance_frame(command.frame, &mut self.world)
                .map_err(unsupported)?;
        } else {
            self.world.throttles = [command.right, command.forward];
            self.component
                .advance_command(command, &mut self.world)
                .map_err(unsupported)?;
            self.ultimate
                .charge(&mut self.component, &mut self.world)
                .map_err(unsupported)?;
            self.modifiers = self
                .component
                .movement_modifiers(&mut self.world)
                .map_err(unsupported)?;
        }
        self.frame = command.frame;
        self.log_assumptions();
        Ok(())
    }

    fn frame_ms(&self) -> u32 {
        FRAME_MS as u32
    }

    fn observer_activity(&self) -> Option<ObserverActivity> {
        let snapshot = self.component.snapshot()?;
        Some(ObserverActivity::of(
            snapshot,
            &self.world.graphs,
            &self.codecs,
        ))
    }

    fn movement_modifiers(&self) -> Option<&BTreeMap<ModifierKey, Modifier>> {
        self.modifiers.as_ref()
    }

    fn projectile_leases(&self) -> Option<&ProjectileLeases> {
        (!self.leases.blocks().is_empty()).then_some(&self.leases)
    }

    fn vitals_mut(&mut self) -> Option<&mut Vitals> {
        Some(&mut self.world.vitals)
    }

    /// driver.py `BodyScript.set_health`: #253 for the graphs, #1732 and
    /// #1737 for the HUD's number, and a body with nothing left is dead to
    /// its own script (the client's LogicalButton takes no presses).
    fn health_changed(&mut self) -> io::Result<()> {
        let (total, maximum) = (self.world.vitals.total(), self.world.vitals.maximum());
        for (variable, value) in [
            (OWNER_HEALTH, total),
            (HUD_HEALTH, total),
            (HUD_MAX_HEALTH, maximum),
        ] {
            self.component
                .set_entity_variable(variable, Value::Float(value), &mut self.world)
                .map_err(unsupported)?;
        }
        self.component.set_owner_dead(total <= 0.0);
        Ok(())
    }

    fn gain_ult(&mut self, points: f64) -> io::Result<()> {
        self.ultimate
            .gain(points, &mut self.component, &mut self.world)
            .map_err(unsupported)
    }

    fn receive_message(&mut self, message: u64, parameters: Variables) -> io::Result<usize> {
        let entries = self
            .component
            .receive_message(message, parameters, &mut self.world)
            .map_err(unsupported)?;
        self.log_assumptions();
        Ok(entries)
    }

    fn take_messages(&mut self) -> Vec<GameMessage> {
        self.world.take_messages()
    }

    fn move_requests(&self) -> Vec<MoveRequest> {
        self.component.move_requests()
    }

    /// match.py `body_command`: the move's end is due at the frame's time.
    fn finish_move(&mut self, request: MoveRequest, arrived: bool, frame: u32) -> io::Result<()> {
        let due = u64::from(frame) * FRAME_MS;
        let delay = due.saturating_sub(self.component.now_ms());
        self.component
            .finish_move(
                request.instance,
                request.raw_state,
                arrived,
                delay,
                &mut self.world,
            )
            .map_err(unsupported)
    }

    fn take_motions(&mut self) -> Vec<Motion> {
        self.component.take_motions()
    }

    fn set_target_candidates(&mut self, candidates: &[TargetCandidate]) {
        let targets = self.world.targets.get_or_insert_with(Vec::new);
        targets.clear();
        targets.extend_from_slice(candidates);
    }

    fn begin_spawn_bark(&mut self, now: Duration) -> io::Result<()> {
        if let Some(bark) = begin_spawn_bark(
            &mut self.component,
            &Arc::clone(&self.world.graphs),
            self.world.hero,
            now,
            &mut self.world,
        )
        .map_err(unsupported)?
        {
            self.bark = Some(bark);
        }
        Ok(())
    }

    fn expire_bark(&mut self, now: Duration) -> io::Result<()> {
        if let Some(bark) = &self.bark {
            if bark
                .expire(now, &mut self.component, &mut self.world)
                .map_err(unsupported)?
            {
                self.bark = None;
            }
        }
        Ok(())
    }

    fn take_shots(&mut self, command: Option<Command>, mover: &Mover) -> Vec<PosedShot> {
        let shots = self.component.take_shots().unwrap_or_default();
        let melee = self.melee(command, mover);
        let Some(command) = command else {
            return Vec::new();
        };
        let [x, y, z] = mover.pose().position;
        let origin = [x, y + self.combat.eyes.height(mover.crouched()), z];
        let aim = aim_direction(&command);
        let mut posed: Vec<_> = shots
            .into_iter()
            .filter_map(|shot| {
                let node = self.component.shot_node(&shot)?;
                let mut evaluate = |config: &Json| {
                    self.component
                        .evaluate_shot_config(&shot, config, &mut self.world)
                };
                // The volley's growing spread turns the aim (a guessed seed;
                // `weapon_spread::shot_direction`).
                let spread = shot_direction(&shot, node, aim, &mut evaluate).ok()?;
                let log_spread = || {
                    if spread != aim {
                        log_assumption(
                            shot.graph,
                            Some(node.pos),
                            "STU_239B571E",
                            "spread seeded from the body, instance, state and counter",
                        );
                    }
                };
                let speed = node.field("m_projectileSpeed").unwrap_or(&Json::Null);
                if !speed.is_null() && to_float(&evaluate(speed).ok()?) != 0.0 {
                    let projectiles = projectiles_per_shot(node, &mut evaluate).ok()?;
                    let ballistics =
                        ProjectileBallistics::from_authored(node, &mut evaluate).ok()??;
                    for class in &ballistics.unmodelled {
                        log_assumption(shot.graph, None, class, "flies as if it weren't there");
                    }
                    let rings = pellet_rings(node, &mut evaluate).ok()?;
                    // projectiles.py `fired`: the owner's client shoots a
                    // networked, predicted volley's projectile under its
                    // body's lease, and the server confirms and releases it;
                    // combat.py `_shoot`: any other flies only for its damage.
                    // The volley's aim id (`m_aimID`) changes nothing here:
                    // the server aims from the eye along the command, as it
                    // does for Tracer's pistols, whose volley has one too.
                    let leased = node.is_networked()
                        && local_prediction(node)
                        && predicted_sync(node, &mut evaluate).ok()?
                        && ballistics
                            .definition
                            .and_then(|definition| self.leases.entity(definition, shot.frame, 0))
                            .is_some();
                    if !leased && !ballistics.damaging() {
                        return None;
                    }
                    // The entity it leaves (projectiles.py `fired`): the
                    // Biotic Field canister leaves its field at the
                    // shooter's feet whatever its arc does.
                    let spawn = spawned_definitions(node).first().copied();
                    let field = self
                        .combat
                        .field
                        .filter(|_| leased && spawn == Some(crate::projectiles::FIELD_ENTITY))
                        .map(Box::new);
                    let volleys = self
                        .component
                        .snapshot()
                        .and_then(|snapshot| snapshot.instances.get(&shot.instance))
                        .and_then(|instance| instance.states.get(&shot.raw_state))
                        .map_or(0, |state| match state.kind {
                            StateKind::WeaponVolley { volleys, .. } => volleys,
                            _ => 0,
                        });
                    // A leased volley leaves along the aim, as the other
                    // clients see it; another along the spread.
                    let direction = if leased {
                        aim
                    } else {
                        log_spread();
                        spread
                    };
                    // Pellet rings (Doomfist's Hand Cannon, Roadhog's Scrap
                    // Gun, Torbjörn's secondary, Echo's Tri-Shot): each of
                    // the shot's projectiles flies for its damage, around
                    // the direction (upstream 7cb4018, any sync).
                    let pellets = match rings {
                        Some(rings) => {
                            log_assumption(
                                shot.graph,
                                Some(node.pos),
                                "STU_613BDDB0",
                                "pellets evenly spaced around each ring, the first on the right",
                            );
                            ring_directions(direction, &rings)
                        }
                        None => vec![direction],
                    };
                    return Some(PosedShot::Projectile(Box::new(PosedProjectile {
                        shot,
                        ballistics,
                        origin,
                        direction,
                        leased,
                        spawn,
                        field,
                        feet: [x, y, z],
                        volleys,
                        projectiles,
                        pellets,
                    })));
                }
                let hit = HitDefinition::from_authored(node.field("m_6396149F")?, &mut evaluate)
                    .ok()??;
                if hit.amount <= 0.0 {
                    return None;
                }
                let reach = node.field("m_FEC435C6").unwrap_or(&Json::Null);
                let reach = if reach.is_null() {
                    0.0
                } else {
                    to_float(&evaluate(reach).ok()?)
                };
                log_spread();
                Some(PosedShot::Hitscan(PosedHitscan {
                    source: shot.entity,
                    origin,
                    command,
                    direction: spread,
                    shot_frame: shot.frame,
                    reach,
                    hit,
                }))
            })
            .collect();
        posed.extend(melee);
        posed
    }
}

fn unsupported(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, error.to_string())
}

#[cfg(any(test, feature = "fixtures"))]
pub fn fixture_body_factory() -> (PlayerBodyFactory, OwnerBodySpawn) {
    use serde_json::json;

    let body = 0x0400_0000_0000_0001;
    let hero = Hero {
        guid: SOLDIER,
        name: "Fixture".into(),
        role: "Damage".into(),
        body,
    };
    let graph = json!({
        "index":42,"items":0,"nodes":[],"states":[],"entries":[],"remote":[],
        "sync_vars":[],"containers":{},"unknown_nodes":[],"fields":{},
        "header":{"m_nodesBitCount":4,"m_statesBitCount":4,
            "m_remoteSyncNodesBitCount":4,"m_syncVarsBitCount":4}
    });
    let bundle = json!({"version":1,"missing":[],"graphs":[graph],"bodies":{
        format!("0x{body:016X}"): {"hero": format!("0x{SOLDIER:016X}"), "name":"Fixture",
            "graphs":[{"graph":42,"m_75408640":0}],"manager":42,"weapons":[null,42]}
    }});
    let graphs = Arc::new(GraphStore::from_json(&serde_json::to_vec(&bundle).unwrap()).unwrap());
    let codecs = Arc::new(CodecTable::from_json(r#"{"classes":{}}"#).unwrap());
    let spawn = OwnerBodySpawn {
        controller: 0xa000_0100,
        entity: 0xa000_0101,
        body_filter: crate::scene::team_bit(0).unwrap(),
        slot: 0,
        frame: 100,
        pose: Movement {
            position: [1.0, 2.0, 3.0],
            ..Movement::default()
        },
        quanta: RETAIL_QUANTA,
    };
    let factory = PlayerBodyFactory::prepare(
        graphs,
        codecs,
        PRACTICE_RANGE_MODE,
        &hero,
        None,
        false,
        BodyCombat {
            // combat.py `EYE`, `EYE_CROUCHED`: Soldier: 76's.
            eyes: orpheus_content::heroes::Eyes {
                standing: 1.55,
                crouched: 1.085,
            },
            melee: Arc::new([]),
            field: None,
        },
        Unknowns::for_build(),
        None,
        spawn,
    )
    .unwrap();
    (factory, spawn)
}

#[cfg(any(test, feature = "fixtures"))]
pub fn fixture_hitscan_factory(damage: f64) -> PlayerBodyFactory {
    use serde_json::json;

    fixture_volley_factory(
        20,
        false,
        json!({
            "m_A17BE89B":2,"m_numShotsPerSecond":9,"m_FEC435C6":180,
            "m_6396149F":{"m_modifyHealth":{"m_amount":damage},"m_A872C70E":2}
        }),
    )
}

#[cfg(any(test, feature = "fixtures"))]
pub const FIXTURE_PROJECTILE_DEFINITION: u64 = 0x0400_0000_0000_072c;

#[cfg(any(test, feature = "fixtures"))]
pub fn fixture_projectile_factory(damage: f64) -> PlayerBodyFactory {
    fixture_volley_factory(29, true, fixture_projectile_fields(damage))
}

#[cfg(any(test, feature = "fixtures"))]
fn fixture_projectile_fields(damage: f64) -> Json {
    serde_json::json!({
        "m_A17BE89B":1,"m_numShotsPerSecond":1,"m_FEC435C6":100,
        "m_45271E8C":true,"m_numProjectilesPerShot":1,
        "m_projectileSpeed":50,"m_projectileLifetime":2,
        "m_projectileEntity":{"$":"STUStatescriptWeaponProjectileEntity","m_A9A62FD0":1,
            "m_entityDef":{"$":"STU_8556841E","m_EE729DCB":0,"m_resourceKey":null,
                "m_entityDef":format!("0x{FIXTURE_PROJECTILE_DEFINITION:016X}")}},
        "m_6396149F":{"m_modifyHealth":{"m_amount":damage}},
        "m_D2D11CE9":{"m_modifyHealth":{"m_amount":80.0},"m_0D30E40C":[
            {"m_radius":1,"m_B399CACB":1},
            {"m_radius":3,"m_B399CACB":0.5}]}
    })
}

#[cfg(any(test, feature = "fixtures"))]
fn fixture_volley_factory(button: u8, networked: bool, mut volley: Json) -> PlayerBodyFactory {
    use serde_json::json;

    volley["m_2BBEEAB8"] = json!(true);
    let remote = if networked { json!([2]) } else { json!([]) };
    let graph = json!({
        "index":42,"items":3,"states":[1,2],"entries":[0],"remote":remote,
        "sync_vars":[],"containers":{},"unknown_nodes":[],"fields":{"m_680A2CB2":[0]},
        "header":{"m_nodesBitCount":4,"m_statesBitCount":4,
            "m_remoteSyncNodesBitCount":4,"m_syncVarsBitCount":4},
        "nodes":[
            {"pos":0,"class":"STUStatescriptEntry","uid":0,"rid":0,"remote":-1,
                "state":null,"client_only":false,"server_only":false,"fields":{
                "m_outPlug":{"$":"Plug","links":[[1,"m_beginPlug"]],"m_FF3DAF1E":[]}}},
            {"pos":1,"class":"STUStatescriptStateLogicalButton","uid":1,"rid":1,"remote":-1,
                "state":0,"client_only":false,"server_only":true,"fields":{
                "m_2BBEEAB8":true,"m_logicalButton":button,
                "m_onGoingDownPlug":{"$":"Plug","links":[[2,"m_beginPlug"]],"m_FF3DAF1E":[]}}},
            {"pos":2,"class":"STUStatescriptStateWeaponVolley","uid":2,"rid":2,
                "remote":if networked { 0 } else { -1 },
                "state":1,"client_only":false,"server_only":!networked,"fields":volley}
        ]
    });
    let bundle = json!({"version":1,"missing":[],"graphs":[graph],"bodies":{}});
    let mut factory = fixture_body_factory().0;
    factory.graphs =
        Arc::new(GraphStore::from_json(&serde_json::to_vec(&bundle).unwrap()).unwrap());
    if networked {
        factory.codecs = Arc::new(
            CodecTable::from_json(include_str!("../fixtures/statescript_codecs.json")).unwrap(),
        );
    }
    factory.roots = vec![Root::new(1, 42)];
    factory
}

/// A fixture body that runs one graph (instance 1) with these entity
/// variables besides the body's own.
#[cfg(any(test, feature = "fixtures"))]
pub fn fixture_graph_factory(graph: Json, variables: Variables) -> PlayerBodyFactory {
    use serde_json::json;

    let index = graph["index"]
        .as_u64()
        .and_then(|index| u32::try_from(index).ok())
        .expect("the graph's index");
    let bundle = json!({"version":1,"missing":[],"graphs":[graph],"bodies":{}});
    let mut factory = fixture_body_factory().0;
    factory.graphs =
        Arc::new(GraphStore::from_json(&serde_json::to_vec(&bundle).unwrap()).unwrap());
    factory.roots = vec![Root::new(1, index)];
    factory.variables.extend(variables);
    factory
}

#[cfg(test)]
mod fault_tests;
#[cfg(test)]
mod tests;
