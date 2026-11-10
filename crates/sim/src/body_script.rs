//! Prepared owner statescript for a live body in build 104319.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    sync::Arc,
};

use serde_json::Value as Json;

use crate::{
    combat::{PosedHitscan, PosedProjectile, PosedShot, eye_height},
    hitscan::aim_direction,
    instance::RETAIL_QUANTA,
    pools::{BODY_GRAPH, FALLBACK_POOLS, HealthPools},
    projectiles::{ProjectileLeases, local_prediction, predicted_sync},
    replication::{BodyStep, OwnerBodyRuntime},
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
    vitals::Vitals,
    vm::{Component, MAX_INIT_WORK, Root, Snapshot},
    weapons::{HitDefinition, ProjectileBallistics, authored_number},
    wire::Variables,
};
use orpheus_wire::{
    commands::Command,
    entity::{Component as EntityComponent, Movement},
};

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
}

impl PlayerBodyFactory {
    pub(crate) fn prepare(
        graphs: Arc<GraphStore>,
        codecs: Arc<CodecTable>,
        mode: u64,
        hero: &Hero,
        pools: Option<HealthPools>,
        controller_on_fire: bool,
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
        Ok(PlayerBodyRuntime {
            component,
            world,
            codecs: Arc::clone(&self.codecs),
            modifiers,
            leases,
            frame: spawn.frame,
        })
    }
}

pub struct PlayerBodyRuntime {
    component: Component,
    world: PlayerWorld,
    codecs: Arc<CodecTable>,
    modifiers: Option<BTreeMap<ModifierKey, Modifier>>,
    leases: ProjectileLeases,
    frame: u32,
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
        SnapshotEncoder::new(&self.world.graphs, &self.codecs)
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
            self.modifiers = self
                .component
                .movement_modifiers(&mut self.world)
                .map_err(unsupported)?;
        }
        self.frame = command.frame;
        Ok(())
    }

    fn frame_ms(&self) -> u32 {
        FRAME_MS as u32
    }

    fn movement_modifiers(&self) -> Option<&BTreeMap<ModifierKey, Modifier>> {
        self.modifiers.as_ref()
    }

    fn projectile_leases(&self) -> Option<&ProjectileLeases> {
        (!self.leases.blocks().is_empty()).then_some(&self.leases)
    }

    fn take_shots(&mut self, command: Option<Command>, mover: &Mover) -> Vec<PosedShot> {
        let shots = self.component.take_shots().unwrap_or_default();
        let Some(command) = command else {
            return Vec::new();
        };
        let [x, y, z] = mover.pose().position;
        let origin = [x, y + eye_height(self.world.hero, mover.crouched()), z];
        shots
            .into_iter()
            .filter_map(|shot| {
                let node = self.component.shot_node(&shot)?;
                let mut evaluate = |config: &Json| {
                    self.component
                        .evaluate_shot_config(&shot, config, &mut self.world)
                };
                let speed = node.field("m_projectileSpeed").unwrap_or(&Json::Null);
                if !speed.is_null() && to_float(&evaluate(speed).ok()?) != 0.0 {
                    // The match flight handles one straight, owner-predicted projectile per shot.
                    if !node.is_networked()
                        || !local_prediction(node)
                        || !predicted_sync(node, &mut evaluate).ok()?
                        || node
                            .field("m_projectileMotions")
                            .is_some_and(|v| !v.is_null())
                        || node.field("m_aimID").is_some_and(|v| !v.is_null())
                        || authored_number(
                            node.field("m_numProjectilesPerShot"),
                            1.0,
                            &mut evaluate,
                        )
                        .ok()?
                            != 1.0
                    {
                        return None;
                    }
                    let ballistics =
                        ProjectileBallistics::from_authored(node, &mut evaluate).ok()??;
                    let definition = ballistics.definition?;
                    self.leases.entity(definition, shot.frame, 0)?;
                    if (ballistics.direct.is_none() && ballistics.blast.is_none())
                        || ballistics.direct.is_some_and(|hit| hit.amount <= 0.0)
                        || ballistics
                            .blast
                            .as_ref()
                            .is_some_and(|blast| blast.hit.amount <= 0.0)
                    {
                        return None;
                    }
                    return Some(PosedShot::Projectile(PosedProjectile {
                        shot,
                        ballistics,
                        origin,
                        direction: aim_direction(&command),
                    }));
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
                Some(PosedShot::Hitscan(PosedHitscan {
                    source: shot.entity,
                    origin,
                    command,
                    shot_frame: shot.frame,
                    reach,
                    hit,
                }))
            })
            .collect()
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

#[cfg(test)]
mod tests;
