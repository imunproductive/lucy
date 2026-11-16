//! Prepared owner statescript for a live body in build 104319.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    sync::Arc,
    time::Duration,
};

use serde_json::Value as Json;

use crate::{
    combat::{BodyCombat, PosedHitscan, PosedProjectile, PosedShot},
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
    expr::{team_from_filter, to_float},
    graph::GraphStore,
    player::PlayerWorld,
    snapshot::{SnapshotEncoder, View},
    timeline::FRAME_MS,
    ultimate::{ChargeRates, UltimateCharge},
    vitals::Vitals,
    vm::{
        AuraChange, BodyEffects, Component, GameMessage, MAX_INIT_WORK, Motion, MoveRequest,
        RaisedCriteria, Root, Snapshot, StateKind, TargetCandidate, Unknowns, World,
    },
    weapons::{HitDefinition, ProjectileBallistics, pellet_rings, ray_profile},
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
/// The body's dead flag (`orpheus_script::bot::DEAD_FLAG`, entity variable #478).
const DEAD_FLAG: u32 = orpheus_script::bot::DEAD_FLAG;
const HUD_HEALTH: u32 = 1732;
const HUD_MAX_HEALTH: u32 = 1737;

/// The mode graph's message that tells a body its respawn time (`06FD.025`),
/// and its seconds parameter (`00B7`).
const RESPAWN_MESSAGE: u64 = 0x0240_0000_0000_06fd;
const RESPAWN_SECONDS: u32 = 0x00b7;

#[derive(Clone, Copy)]
pub struct OwnerBodySpawn {
    pub controller: u32,
    pub entity: u32,
    pub body_filter: u64,
    pub slot: u8,
    pub frame: u32,
    pub pose: Movement,
    pub quanta: u32,
    /// The match's value of the mode entity's component 114 field 9
    /// ([`crate::scene::mode_create`]), which the mode body script `0C8E`
    /// reads (`STU_B182C5C9`).
    pub mode_random: f32,
}

/// What the game mode adds to each player body: its team's body script
/// and the respawn time the mode graph tells it.
///
/// Retail: each team of a mode (`0C5` `m_teams`) names a graph every body
/// of the team runs, `m_bodyScript` (`0C8E` for the PvP modes and the
/// Practice Range `005A`). The mode graph (`13C0`, `0F4A`, `0EC2`) sends
/// every player body `06FD.025` with the respawn seconds (`00B7`) from a
/// SendGameMessage state (`STU_38EE1100`, `13C0` st15) that runs the whole
/// match. `0C8E` waits for that message (st2) before it watches the dead
/// flag #478 (st0), and on death counts the respawn time down in #384
/// (ChaseVar st1) with the HUD stacks #6034 (seconds) and #6036 (share
/// left). See `D:/Work/Coding/refs/reports/DeathCam.md`.
///
/// The server's mode graph doesn't drive the match yet (its gaps,
/// `orpheus_script::mode`), so nothing delivers its messages to every
/// entity (`STU_1D719351`), and the body takes `06FD` from the mode as it
/// starts. Assumptions: the mode body
/// script is the body's last root instance, after the hero's weapons (the
/// instance order retail uses is not known); `06FD` comes on the body's
/// first frame, with `00B7` alone (`13C0` sends only that; `0F4A`'s other
/// parameters are left out).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModeBody {
    /// `m_bodyScript` by team (`TeamIndex`, as the body's filter bits give
    /// it).
    pub scripts: BTreeMap<i64, u32>,
    /// The seconds of the mode graph's `06FD.025`, when it sends one.
    pub respawn_seconds: Option<i64>,
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
    mode_body: ModeBody,
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
        mode_body: ModeBody,
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
        let mut factory = Self {
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
            mode_body,
        };
        // A mode body script the server can't run (refused content in a
        // debug build) leaves the body without it, as a mode graph that
        // doesn't run whole leaves the mode entity to the match flow
        // (`crate::mode_script`).
        if let Err(error) = factory.probe(probe) {
            if factory.mode_body.scripts.is_empty() {
                return Err(error);
            }
            eprintln!(
                "mode {mode:016X}: {} runs without the mode body scripts {:04X?} ({error})",
                hero.name,
                factory.mode_body.scripts.values().collect::<Vec<_>>()
            );
            factory.mode_body = ModeBody::default();
            factory.probe(probe)?;
        }
        Ok(factory)
    }

    /// Start a body and project its first owner frame, as the server would.
    fn probe(&self, spawn: OwnerBodySpawn) -> io::Result<()> {
        let runtime = self.start(spawn)?;
        let encoder = runtime.encoder();
        let snapshot = runtime
            .snapshot()
            .expect("successful body startup has a snapshot");
        let projected = encoder
            .project(snapshot, View::Owner)
            .map_err(unsupported)?;
        encoder.full(&projected).map_err(unsupported)?;
        Ok(())
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
        world.mode_random = Some(spawn.mode_random);
        world.movement = spawn.pose;
        if !has_authored_pools {
            world.vitals = self.pools.unwrap_or(FALLBACK_POOLS).vitals();
        }
        world.pool_types_known = self.pools.is_some() || has_authored_pools;
        world.target_tags = self.target_tags.clone();
        world.controller_on_fire = self.controller_on_fire;
        world.unknowns = self.unknowns;
        let mut roots = self.roots.clone();
        let mode_script = self
            .mode_body
            .scripts
            .get(&team_from_filter(spawn.body_filter))
            .copied();
        if let Some(graph) = mode_script {
            let id = roots.last().map_or(Some(1), |root| root.id.checked_add(1));
            let id = id.ok_or_else(|| unsupported("no instance id left for the mode body"))?;
            roots.push(Root::new(id, graph));
        }
        let mut component = Component::start(
            Arc::clone(&self.graphs),
            spawn.entity,
            self.variables.clone(),
            roots,
            spawn.frame,
            &mut world,
            MAX_INIT_WORK,
        )
        .map_err(unsupported)?;
        component
            .advance_frame(spawn.frame, &mut world)
            .map_err(unsupported)?;
        if let (Some(_), Some(seconds)) = (mode_script, self.mode_body.respawn_seconds) {
            component
                .receive_message(
                    0,
                    RESPAWN_MESSAGE,
                    Variables::from([(RESPAWN_SECONDS, Value::Integer(seconds))]),
                    &mut world,
                )
                .map_err(unsupported)?;
        }
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
    /// The body's eyes and the Biotic Field.
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
        self.world.eye_height = Some(self.combat.eyes.height(mover.crouched()));
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
    /// its own script (the client's LogicalButton takes no presses). A body
    /// that dies also raises its dead flag #478: no graph writes it, and
    /// the graphs that show a body to others read it (`0D24`'s health bar
    /// and name tag, `01CF`), so the server sets it as it does for a dying
    /// bot (assumption: retail did so for every body that died). A body
    /// can't come back to life; a respawn is a new body.
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
        let dead = total <= 0.0;
        if dead {
            self.component
                .set_entity_variable(DEAD_FLAG, Value::Bool(true), &mut self.world)
                .map_err(unsupported)?;
        }
        self.component.set_owner_dead(dead);
        Ok(())
    }

    fn change_aura(&mut self, change: &AuraChange) -> io::Result<()> {
        self.component
            .change_aura(change, &mut self.world)
            .map_err(unsupported)?;
        self.log_assumptions();
        Ok(())
    }

    fn take_aura_changes(&mut self) -> Vec<AuraChange> {
        self.world.take_aura_changes()
    }

    fn effects(&self) -> Option<&BodyEffects> {
        Some(&self.world.effects)
    }

    fn gain_ult(&mut self, points: f64) -> io::Result<()> {
        self.ultimate
            .gain(points, &mut self.component, &mut self.world)
            .map_err(unsupported)
    }

    fn receive_message(
        &mut self,
        sender: u32,
        message: u64,
        parameters: Variables,
    ) -> io::Result<usize> {
        let entries = self
            .component
            .receive_message(sender, message, parameters, &mut self.world)
            .map_err(unsupported)?;
        self.log_assumptions();
        Ok(entries)
    }

    fn take_messages(&mut self) -> Vec<GameMessage> {
        self.world.take_messages()
    }

    fn take_criteria(&mut self) -> Vec<RaisedCriteria> {
        self.world.take_criteria()
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

    /// A hero setting is an entity variable no graph writes. A changed one
    /// queues a re-evaluation of the states that watch it (`01CF`
    /// st115/st116), which runs now, so the next frame carries the result.
    fn set_hero_settings(&mut self, variables: &Variables) -> io::Result<()> {
        for (&id, value) in variables {
            self.component
                .set_entity_variable(id, value.clone(), &mut self.world)
                .map_err(unsupported)?;
        }
        self.component.run_now(&mut self.world).map_err(unsupported)
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
        // Its graphs' health changes (quick melee `0043` st4), as its shots
        // only once its script is complete.
        let health: Vec<_> = self
            .world
            .take_health_changes()
            .into_iter()
            .map(PosedShot::Health)
            .collect();
        let Some(shots) = self.component.take_shots() else {
            return Vec::new();
        };
        let Some(command) = command else {
            return health;
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
                    // A networked volley the client doesn't shoot itself,
                    // which flies for its damage: the server shows its one
                    // projectile to every client, its shooter's included
                    // (`projectiles::ServerCopies`; beyond Python).
                    let server_shown = !leased
                        && node.is_networked()
                        && !local_prediction(node)
                        && ballistics.definition.is_some()
                        && spawn.is_none()
                        && rings.is_none()
                        && projectiles == 1;
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
                    // A leased or server-shown volley leaves along the aim,
                    // as the clients see it; another along the spread.
                    let direction = if leased || server_shown {
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
                        server_shown,
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
                    ray_profile: ray_profile(node, &mut evaluate),
                }))
            })
            .collect();
        posed.extend(health);
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
        mode_random: 0.5,
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
            field: None,
        },
        Unknowns::for_build(),
        None,
        ModeBody::default(),
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
