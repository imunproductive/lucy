//! Load the authorized data needed before the game worker can create a world.
//!
//! Any map of the catalog: its collision comes from the cache folder, built
//! from the install first when a cache is missing or of another version
//! (ow174/game/collision.py `_read_cache`): the movement cache the movers
//! and probes meet, and the ray cache with every category, which the shots
//! and projectiles meet ([`PreparedScene::hitscan_wall`]).

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{self, File},
    io::{self, BufReader, Read},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::Deserialize;

use crate::{
    body_script::{BodyMap, ModeBody, OwnerBodySpawn, PlayerBodyFactory},
    bot_animation::{BotRigCatalog, ScriptAnims},
    bot_plate::{BotPlate, PlateSample},
    bot_script::{BotEvent, BotScripts, BotShot},
    bots::{Bot, BotCatalog, BotWalk},
    combat::{BodyCombat, BotDeath, RespawnTimes},
    controller::Controller,
    hitboxes::HitboxCatalog,
    hitscan::{RAY_PROFILES_FILE, RayProfiles, SceneRays},
    hurt_feedback::HurtFeedback,
    map_areas::{self, DamageArea, MapAreaData, PlacedScript, SpawnRoom},
    match_flow::{MatchFlow, Mode, Timers},
    messages::Bots,
    mode_graph_values::ModeGraphValues,
    mode_rules_data::{ModeRules, ModeRulesData},
    notices::switch_notices,
    objective_data::{MapObjectives, ObjectiveData},
    objectives::CaptureBox,
    placed_script::PlacedScripts,
    pools::PoolCatalog,
    projectiles::FIELD_ENTITY,
    replication::OwnerBodyRuntime,
    scene::{PlayerAppearance, initial_world, mode_ui, placeable_components},
};
use orpheus_content::{
    heroes::{EyeCatalog, Hero, HeroCatalog, SOLDIER},
    maps::{GameMap, GameModes, MODES_FILE, SpawnChoice},
    stats::{BoardStats, PlayerStats, StatRules, StatsCatalog, initial_on_fire},
    voice_sets::VoiceCatalog,
};
use orpheus_core::{bits::BitWriter, guid::parse_guid};
use orpheus_physics::{
    collision::cache::{
        MAGIC, RayCacheIdentity, VERSION, chunk_guid, read_movement_cache, read_ray_cache,
    },
    collision::index::TriangleIndex,
    collision::scene::CollisionScene,
    contact::cache::CapsulePose,
    contact::world::ContactWorld,
    mover::Mover,
    mover::ground::NOT_GROUND,
    mover::settings::{MoverCatalog, MoverSettings},
    nav::{self, Navigation},
};
use orpheus_script::{
    codecs::CodecTable,
    graph::GraphStore,
    map_tags::MapTags,
    placed::{DummyChange, NextDummy},
    vitals::Vitals,
    vm::{GameMessage, MapRays, MessageEnd, TargetCandidate, Unknowns},
    wire::Variables,
};
use orpheus_wire::entity::{Component, EntityUpdate, Operation};

const MOVER_CATEGORY: u16 = 0x18;

fn mover_category(map: &GameMap, team: u8) -> u16 {
    match (map.free_for_all, team) {
        (false, 0) => 0x08,
        (false, 1) => 0x10,
        _ => MOVER_CATEGORY,
    }
}

/// The body's mover a bot flies on (a knockback, a velocity action), made
/// at its feet and facing as of `tick` when it takes off; none without
/// player collision. The training bots' bodies have a character mover as
/// the heroes' have (`STUCharacterMoverComponent`).
fn bot_mover<'a>(
    contacts: Option<&'a Arc<ContactWorld>>,
    movers: &MoverCatalog,
    map: &GameMap,
    bot: &Bot,
    tick: u32,
) -> impl FnOnce() -> Option<Mover> + 'a {
    let settings = *movers.for_body(Some(bot.spawn.body));
    let category = mover_category(map, bot.spawn.team);
    let (position, yaw) = (bot.position, bot.movement(tick).yaw);
    move || Mover::new(settings, Arc::clone(contacts?), position, yaw, category).ok()
}

fn ground_height(
    ground: &TriangleIndex,
    category: u16,
) -> impl Fn(f64, f64, f64, f64) -> Option<f64> + '_ {
    move |x, z, top, bottom| {
        ground
            .ground(x, z, top, bottom, category)
            .filter(|hit| hit.surface & NOT_GROUND == 0)
            .map(|hit| hit.y)
    }
}

/// Starts a player body: its hero, its spawn and the map its scripts meet
/// ([`PreparedScene::body_map`]).
type OwnerBodyFactory = dyn Fn(&Hero, OwnerBodySpawn, BodyMap) -> io::Result<Option<Box<dyn OwnerBodyRuntime>>>
    + Send
    + Sync;

/// A map's two collision cache files: the movement cache
/// (`orpheus_assets::collision_cache`) and the ray cache the shots meet
/// (`orpheus_assets::ray_cache`).
pub struct CollisionFiles {
    pub movement: Vec<u8>,
    pub rays: Vec<u8>,
}

/// Builds a map's collision cache files from the install: (map GUID, the
/// server's map name) to the files' bytes.
pub type CollisionBuild = dyn Fn(u64, &str) -> io::Result<CollisionFiles> + Send + Sync;

pub struct SceneAssets {
    collision: PathBuf,
    /// The runtime folder, whose `data/navigation` has each map's AI
    /// navigation (`orpheus_physics::nav::file`).
    runtime: PathBuf,
    build_collision: Option<Arc<CollisionBuild>>,
    mode_props: BTreeMap<u64, BTreeSet<u64>>,
    graphs: Arc<GraphStore>,
    codecs: Option<Arc<CodecTable>>,
    /// Each mode's owner body factory, made the first time a scene of the
    /// mode is prepared.
    owner_bodies: Mutex<HashMap<u64, Option<Arc<OwnerBodyFactory>>>>,
    bot_graphs: Option<Arc<GraphStore>>,
    bot_unknowns: Unknowns,
    player_unknowns: Unknowns,
    /// Players' bodies' ultimate generation variables; None: the rulesets'
    /// (`orpheus_script::ultimate::generation_variables`).
    ult_generation: Option<orpheus_script::wire::Variables>,
    bots: BotCatalog,
    movers: Arc<MoverCatalog>,
    eyes: Arc<EyeCatalog>,
    /// Each body's voice set (`game_voice_sets_174.json`).
    voice_sets: Arc<VoiceCatalog>,
    /// Each body's hit shapes (`game_hitboxes_174.json`).
    hitboxes: Arc<HitboxCatalog>,
    /// The bot bodies' rigs (`game_bot_animation_174.json`).
    bot_rigs: BotRigCatalog,
    /// The bots' graphs' states that animate them.
    script_anims: Arc<ScriptAnims>,
    respawns: RespawnTimes,
    /// The mode graphs' values the match flow uses.
    mode_graph_values: ModeGraphValues,
    /// The Biotic Field's initial graphs; without them no field runs.
    field_graphs: Vec<u32>,
    stats: Arc<StatsCatalog>,
    /// The stats the server counts (the Tab board's player list).
    board: BoardStats,
    heroes: Arc<HeroCatalog>,
    pools: Arc<PoolCatalog>,
    /// The game modes' teams (`game_modes_174.json`).
    modes: GameModes,
    /// The match flow's values the graphs and objectives hold.
    mode_rules: ModeRulesData,
    /// Each map's objective data (`game_objectives_174.json` `maps`).
    objectives: ObjectiveData,
    /// Each ray profile's map mask (`game_ray_profiles_174.json`).
    ray_profiles: Arc<RayProfiles>,
    /// Each map's spawn rooms, damage areas and server placeables
    /// (`game_map_areas_174.json`).
    map_areas: MapAreaData,
    /// Each body definition's target tags ([`body_tags`]).
    body_tags: Arc<BodyTags>,
}

/// Why the environment kills a body ([`PreparedScene::fell_out`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FellOut {
    /// A damage area, kill volume or catch floor of the map.
    Environment,
    /// The safety net below the whole map: the map's data lacks the volume
    /// that kills there.
    BelowMap,
}

pub struct PreparedScene {
    map: GameMap,
    ground: CollisionScene,
    /// The map's collision as the shots and projectiles meet it: every
    /// category (the ray cache), queried with the volley's ray profile's map
    /// mask ([`PreparedScene::hitscan_wall`]).
    rays: Arc<CollisionScene>,
    ray_profiles: Arc<RayProfiles>,
    contacts: Option<Arc<ContactWorld>>,
    controller: Controller,
    mode_ui: BitWriter,
    graphs: Arc<GraphStore>,
    initial_board: Component,
    bots: Vec<Bot>,
    bot_scripts: BotScripts,
    /// The live nameplate tool's changes to the bots (`bot_plate`).
    bot_plate: Arc<BotPlate>,
    /// The live hurt feedback switch (`hurt_feedback`).
    hurt: HurtFeedback,
    script_anims: Arc<ScriptAnims>,
    movers: Arc<MoverCatalog>,
    eyes: Arc<EyeCatalog>,
    voice_sets: Arc<VoiceCatalog>,
    hitboxes: Arc<HitboxCatalog>,
    /// The mode's respawn time for heroes.
    respawn: Duration,
    heroes: Arc<HeroCatalog>,
    pools: Arc<PoolCatalog>,
    stats: Arc<StatsCatalog>,
    board: BoardStats,
    pub owner_body_factory: Option<Arc<OwnerBodyFactory>>,
    /// The Biotic Field's initial graphs (`03DC.003` starts `0258`).
    field_graphs: Vec<u32>,
    /// The mode posts hero switches in the chat (notices.py).
    switch_notices: bool,
    /// The map's AI navigation, when the data has it (Practice vs AI).
    navigation: Option<Arc<Navigation>>,
    /// The rules of the mode's match flow.
    mode_rules: ModeRules,
    /// The mode graphs' values the match flow uses.
    mode_graph_values: ModeGraphValues,
    /// The map's objective data.
    objectives: Arc<MapObjectives>,
    /// The map's spawn rooms.
    spawn_rooms: Arc<[SpawnRoom]>,
    /// The map's damage areas that kill, in this mode.
    lethal_areas: Arc<[DamageArea]>,
    /// The map's kill volumes (`map_areas` module notes).
    kill_volumes: Arc<[CaptureBox]>,
    /// The map's server placeables in this mode: server-owned placeables
    /// the loading screen doesn't wait for, created after the player and
    /// the mode, with their script data.
    server_placeables: Vec<Arc<PlacedScript>>,
    /// The map's entities with target tags in this mode, which the scene
    /// tag queries find (`map_areas::MapAreaData::map_tags`).
    map_tags: Arc<MapTags>,
    /// Each body definition's target tags ([`body_tags`]).
    body_tags: Arc<BodyTags>,
    /// Their scripts (`placed_script`).
    placed_scripts: PlacedScripts,
    /// Where a falling body dies on a map with neither damage areas nor
    /// kill volumes (`map_areas::kill_height`).
    kill_height: Option<f64>,
    /// [`map_areas::FALL_LIMIT`] below the map's lowest collision triangle:
    /// the safety net under the whole map (`map_areas` module notes).
    safety_net: f64,
    /// The match's value of the mode entity's component 114 field 9
    /// ([`crate::scene::mode_create`]), drawn once per match.
    mode_random: f32,
}

impl SceneAssets {
    pub fn load(runtime: impl AsRef<Path>) -> io::Result<Self> {
        let runtime = runtime.as_ref();
        let data = runtime.join("data");
        let graphs = Arc::new(GraphStore::load_runtime(&data, "game_graphs_174.json.gz")?);
        // The bot bundle adds the bots' bodies to the hero bundle; the heroes
        // keep the hero bundle, which has their target tags.
        let bot_graphs = match GraphStore::load_runtime(&data, "game_graphs_bots_174.json.gz") {
            Ok(graphs) => Some(Arc::new(graphs)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let codecs = Some(Arc::new(CodecTable::client()));
        let pools = Arc::new(PoolCatalog::from_graphs(&graphs)?);
        let heroes = HeroCatalog::load(data.join("game_heroes_174.json"))?;
        // A body the movers or eyes don't list is the server's fallback
        // hero's (mover.py `SOLDIER_BODY`).
        let fallback = heroes
            .get(SOLDIER)
            .ok_or_else(|| invalid("the hero catalog has no Soldier: 76"))?
            .body;
        let field_graphs =
            field_graphs(&data.join("game_spawns_174.json")).unwrap_or_else(|error| {
                eprintln!("the Biotic Field is unavailable ({error})");
                Vec::new()
            });
        let stats = Arc::new(
            StatsCatalog::load(data.join("game_stats_174.json"))?
                .with_rules(StatRules::load(data.join("game_stat_rules_174.json"))?),
        );
        let board = stats.board()?;
        let mode_rules = ModeRulesData::load(&graphs, &data)?;
        let objectives = ObjectiveData::load(&data)?;
        let map_areas = MapAreaData::load(&data)?;
        let body_tags = Arc::new(body_tags(&graphs, bot_graphs.as_deref()));
        let script_anims = Arc::new(
            bot_graphs
                .as_deref()
                .map(ScriptAnims::from_graphs)
                .unwrap_or_default(),
        );
        Ok(Self {
            collision: runtime.join("collision"),
            runtime: runtime.to_path_buf(),
            build_collision: None,
            mode_props: read_mode_props(&data.join("collision_props_174.json"))?,
            mode_graph_values: ModeGraphValues::read(&graphs)?,
            field_graphs,
            graphs,
            codecs,
            owner_bodies: Mutex::default(),
            bot_graphs,
            bot_unknowns: Unknowns::for_build(),
            player_unknowns: Unknowns::for_build(),
            ult_generation: None,
            bots: BotCatalog::load(data.join("ai_spawners_174.json"))?,
            movers: Arc::new(MoverCatalog::load(
                data.join("game_movers_174.json"),
                fallback,
            )?),
            eyes: Arc::new(EyeCatalog::load(data.join("game_eyes_174.json"), fallback)?),
            voice_sets: Arc::new(VoiceCatalog::load(data.join("game_voice_sets_174.json"))?),
            hitboxes: Arc::new(HitboxCatalog::load(data.join("game_hitboxes_174.json"))?),
            bot_rigs: BotRigCatalog::load(data.join("game_bot_animation_174.json"))?,
            script_anims,
            respawns: RespawnTimes::load(data.join("game_respawns_174.json"))?,
            stats,
            board,
            heroes: Arc::new(heroes),
            pools,
            modes: GameModes::load(data.join(MODES_FILE))?,
            mode_rules,
            objectives,
            ray_profiles: Arc::new(RayProfiles::load(data.join(RAY_PROFILES_FILE))?),
            map_areas,
            body_tags,
        })
    }

    /// Build missing or outdated collision caches from the install.
    pub fn with_collision_build(mut self, build: Arc<CollisionBuild>) -> Self {
        self.build_collision = Some(build);
        self
    }

    /// Keep the collision caches in `folder` instead of the data's own.
    pub fn with_collision_folder(mut self, folder: impl Into<PathBuf>) -> Self {
        self.collision = folder.into();
        self
    }

    pub fn graphs(&self) -> &GraphStore {
        &self.graphs
    }

    pub fn heroes(&self) -> &HeroCatalog {
        &self.heroes
    }

    /// The bot graph bundle (`game_graphs_bots_174.json.gz`), when the data
    /// has one: the bots' bodies and their AI spawner graphs.
    pub fn bot_graphs(&self) -> Option<&GraphStore> {
        self.bot_graphs.as_deref()
    }

    /// How the bot scripts treat unknown statescript content. By default as
    /// every script does ([`Unknowns::for_build`]); the stock bot graphs
    /// need it tolerated, as the Python reference did.
    pub fn set_bot_unknowns(&mut self, unknowns: Unknowns) {
        self.bot_unknowns = unknowns;
    }

    /// How the players' body scripts treat unknown statescript content. By
    /// default as every script does ([`Unknowns::for_build`]); tests of a
    /// hero whose graphs still have gaps tolerate them as release builds do.
    pub fn set_player_unknowns(&mut self, unknowns: Unknowns) {
        self.player_unknowns = unknowns;
    }

    /// The ultimate generation variables of players' bodies, which `0B8B`
    /// scales its charge by: by default each hero's and mode's ruleset
    /// settings; tests of an ultimate fill it sooner.
    pub fn set_ult_generation(&mut self, variables: orpheus_script::wire::Variables) {
        self.ult_generation = Some(variables);
    }

    /// Load the map before handing it to the worker. The render layouts and
    /// graph requirements are checked without blocking a game tick. A map's
    /// collision is built from the install when its caches are missing or
    /// outdated, which can take a while: callers prepare off the game thread
    /// (match.py `_refresh_collision`).
    pub fn prepare(&self, map: GameMap) -> io::Result<PreparedScene> {
        let index = map.map_guid as u32;
        let path = self.collision.join(format!("{index:04X}.col"));
        let rays_path = self.collision.join(format!("{index:04X}.rays.col"));
        self.ensure_collision(&map, [&path, &rays_path])?;
        let cache = read_movement_cache(BufReader::new(File::open(path)?)).map_err(invalid)?;
        for (field, expected) in [("map", map.map_guid), ("chunk", chunk_guid(map.map_guid))] {
            if let Some(value) = cache.header.source.get(field) {
                if value.as_str().and_then(parse_guid) != Some(expected) {
                    return Err(invalid("movement cache identifies a different map"));
                }
            }
        }
        let empty = BTreeSet::new();
        let identifiers = self.mode_props.get(&map.mode_guid).unwrap_or(&empty);
        let ground = CollisionScene::movement_probe(&cache, identifiers).map_err(invalid)?;
        // The ray cache is of the movement cache's chunk (both are built
        // together).
        let ckey = cache
            .header
            .source
            .get("ckey")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| invalid("movement cache without its chunk's content key"))?;
        let identity = RayCacheIdentity {
            map_guid: map.map_guid,
            collision_ckey: ckey,
            placement_ckey: None,
            prop_records_sha256: None,
        };
        let rays = {
            let cache = read_ray_cache(BufReader::new(File::open(rays_path)?), identity)
                .map_err(invalid)?;
            Arc::new(CollisionScene::expanded_rays(&cache, identifiers, identity).map_err(invalid)?)
        };
        let mut bots = self.bots.make(map.map_guid, |body| {
            BotWalk::from_mover(self.movers.for_body(Some(body)))
        });
        // Every map's players move on its collision (match.py `new_mover`).
        let contacts = Some(
            ContactWorld::from_movement_cache(cache, identifiers)
                .map(Arc::new)
                .map_err(invalid)?,
        );
        if let Some(world) = &contacts {
            for bot in bots.iter_mut().filter(|bot| !bot.spawn.route.is_empty()) {
                let category = mover_category(&map, bot.spawn.team);
                bot.attach_collision(
                    self.movers.for_body(Some(bot.spawn.body)),
                    Arc::clone(world),
                    category,
                );
            }
        }
        for bot in &mut bots {
            if let Some(body) = self.bot_rigs.for_body(bot.spawn.body) {
                bot.attach_animation(body);
            }
        }
        let map_rays = SceneRays::shared(&rays, &self.ray_profiles);
        let bot_scripts = BotScripts::new(
            self.bot_graphs.clone(),
            self.codecs
                .clone()
                .unwrap_or_else(|| Arc::new(CodecTable::client())),
            self.bot_unknowns,
            Arc::clone(&self.voice_sets),
            &map,
            &bots,
            Arc::clone(&map_rays),
        );
        let controller = Controller::load(
            &self.graphs,
            map.mode_guid,
            self.codecs
                .clone()
                .unwrap_or_else(|| Arc::new(CodecTable::client())),
        )?
        .with_unknowns(self.player_unknowns);
        let mode_ui = mode_ui(&self.graphs, map.mode_guid, &Variables::new())?;
        let initial_board =
            self.stats
                .component(&PlayerStats::default(), map.mode_guid, None, None);
        let map_mode = map.mode_guid;
        let navigation = self.navigation(&map);
        let mode_random = crate::scene::draw_mode_random();
        let mode_rules =
            self.mode_rules
                .rules(self.graphs.rulesets(), map.mode_guid, mode_random)?;
        let objectives = self.objectives.map(&map)?;
        let spawn_rooms = self.map_areas.spawn_rooms(&map);
        let lethal_areas = self.map_areas.lethal_areas(&map, identifiers);
        let kill_volumes = self.map_areas.kill_volumes(&map);
        // The client shows a server-owned placeable only once the server
        // creates it.
        let server_placeables = self.map_areas.placed_scripts(&map, identifiers);
        let map_tags = self.map_areas.map_tags(&map, identifiers, &self.graphs);
        let lowest_spawn = map
            .spawns
            .iter()
            .chain(map.rooms.iter().map(|room| &room.spawn))
            .map(|spawn| spawn.position[1])
            .min_by(f64::total_cmp);
        let kill_height = lowest_spawn
            .filter(|_| lethal_areas.is_empty() && kill_volumes.is_empty())
            .and_then(|below| map_areas::kill_height(ground.triangles().triangles(), below));
        let safety_net = ground.triangles().lowest() - map_areas::FALL_LIMIT;
        // A mode that plays for objectives needs their data, checked here
        // rather than when the match begins.
        if Mode::of(map.mode_guid).is_some_and(Mode::has_objectives)
            && MatchFlow::new(
                &map,
                false,
                self.mode_graph_values,
                mode_rules,
                Arc::clone(&objectives),
                Timers::default(),
            )
            .is_none()
        {
            return Err(invalid(format!(
                "{}: the objective data doesn't fit the map's objectives in mode {:016X}",
                map.name, map.mode_guid
            )));
        }
        let placed_scripts = PlacedScripts::new(
            Arc::clone(&self.graphs),
            map_mode,
            mode_random,
            server_placeables.clone(),
            Arc::clone(&map_tags),
            map_rays,
            Arc::clone(&spawn_rooms),
        );
        let mut scene = PreparedScene {
            respawn: self.respawns.for_mode(map.mode_guid),
            map,
            ground,
            rays,
            ray_profiles: Arc::clone(&self.ray_profiles),
            contacts,
            controller,
            mode_ui,
            graphs: Arc::clone(&self.graphs),
            initial_board,
            bots,
            bot_scripts,
            bot_plate: Arc::default(),
            hurt: HurtFeedback::default(),
            script_anims: Arc::clone(&self.script_anims),
            movers: Arc::clone(&self.movers),
            eyes: Arc::clone(&self.eyes),
            voice_sets: Arc::clone(&self.voice_sets),
            hitboxes: Arc::clone(&self.hitboxes),
            heroes: Arc::clone(&self.heroes),
            pools: Arc::clone(&self.pools),
            stats: Arc::clone(&self.stats),
            board: self.board,
            owner_body_factory: None,
            field_graphs: self.field_graphs.clone(),
            switch_notices: switch_notices(self.modes.get(map_mode)),
            navigation,
            mode_rules,
            mode_graph_values: self.mode_graph_values,
            objectives,
            spawn_rooms,
            lethal_areas,
            kill_volumes,
            kill_height,
            safety_net,
            mode_random,
            placed_scripts,
            server_placeables,
            map_tags,
            body_tags: Arc::clone(&self.body_tags),
        };
        let mut factories = self
            .owner_bodies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        scene.owner_body_factory = factories
            .entry(scene.map.mode_guid)
            .or_insert_with(|| self.prepare_owner_body(&scene))
            .clone();
        Ok(scene)
    }

    /// Build the map's caches from the install when either is missing or of
    /// another version, one build at a time (collision.py `_read_cache`).
    /// Without an install the caches must be there already.
    fn ensure_collision(&self, map: &GameMap, paths: [&Path; 2]) -> io::Result<()> {
        let Some(build) = &self.build_collision else {
            return Ok(());
        };
        let ready = || {
            paths
                .iter()
                .all(|path| cache_version(path) == Some(VERSION))
        };
        if ready() {
            return Ok(());
        }
        // One cache build at a time in the process (collision.py
        // `_building`, a module lock), whichever assets ask: two sets that
        // share a cache folder, as tests side by side do, build a missing map
        // once rather than both into the same file.
        static BUILDING: Mutex<()> = Mutex::new(());
        let _building = BUILDING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if ready() {
            return Ok(());
        }
        let files = build(map.map_guid, &map.name)?;
        fs::create_dir_all(&self.collision)?;
        for (path, bytes) in paths.into_iter().zip([files.movement, files.rays]) {
            let temporary = path.with_extension("col.tmp");
            fs::write(&temporary, bytes)?;
            fs::rename(&temporary, path)?;
        }
        Ok(())
    }

    /// The map's AI navigation; none when the data has no file for it, as
    /// derivations before version 7 (`orpheus_physics::nav`).
    fn navigation(&self, map: &GameMap) -> Option<Arc<Navigation>> {
        match Navigation::load(nav::file(&self.runtime, map.map_guid)) {
            Ok(navigation) => Some(Arc::new(navigation)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                eprintln!("{}: no AI navigation ({error})", map.name);
                None
            }
        }
    }

    /// Every hero's owner body script, as Python's `BODY_SCRIPT_HEROES = None`
    /// (`ow174/game/match.py`) runs them all. Each hero's graphs are prepared
    /// the first time someone picks it, with that first spawn as the probe;
    /// a hero whose script can't start gets a body without one.
    fn prepare_owner_body(&self, scene: &PreparedScene) -> Option<Arc<OwnerBodyFactory>> {
        let graphs = Arc::clone(&self.graphs);
        let pools = Arc::clone(&self.pools);
        let eyes = Arc::clone(&self.eyes);
        let player_unknowns = self.player_unknowns;
        let ult_generation = self.ult_generation.clone();
        let mode = scene.map.mode_guid;
        let mode_body = ModeBody {
            scripts: self
                .modes
                .get(mode)
                .into_iter()
                .flat_map(|mode| &mode.teams)
                .filter_map(|team| {
                    Some((
                        team.team,
                        u32::try_from(team.body? & 0xFFFF_FFFF_FFFF).ok()?,
                    ))
                })
                .collect(),
            // The Range's `13C0` holds its own `06FD` to every body.
            respawn_seconds: self
                .respawns
                .sent(mode)
                .filter(|_| !crate::mode_script::without_flow(mode))
                .map(|time| time.as_secs() as i64),
            team_sizes: crate::instance::team_entries(scene.map()),
        };
        let ready = self
            .codecs
            .clone()
            .ok_or_else(|| "the owner statescript codec table is unavailable".to_owned())
            .and_then(|codecs| {
                let on_fire = initial_on_fire(&scene.initial_board).ok_or_else(|| {
                    "the controller's initial on-fire flag is unavailable".to_owned()
                })?;
                Ok((codecs, on_fire))
            });
        let prepared: Mutex<HashMap<u64, Result<Arc<PlayerBodyFactory>, String>>> =
            Mutex::default();
        Some(Arc::new(move |hero, spawn, map| {
            let factory = prepared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(hero.guid)
                .or_insert_with(|| {
                    let (codecs, on_fire) = ready.clone()?;
                    let factory = PlayerBodyFactory::prepare(
                        Arc::clone(&graphs),
                        codecs,
                        mode,
                        hero,
                        pools.hero_pools(hero.guid),
                        on_fire,
                        BodyCombat {
                            eyes: eyes.for_body(hero.body),
                        },
                        player_unknowns,
                        ult_generation.clone(),
                        mode_body.clone(),
                        spawn,
                    )
                    .map(Arc::new)
                    .map_err(|error| error.to_string());
                    if let Err(reason) = &factory {
                        eprintln!("{} owner statescript unavailable ({reason})", hero.name);
                    }
                    factory
                })
                .clone()
                .map_err(|reason| io::Error::new(io::ErrorKind::Unsupported, reason))?;
            let runtime: Box<dyn OwnerBodyRuntime> = Box::new(factory.start_on(spawn, Some(map))?);
            Ok(Some(runtime))
        }))
    }
}

impl PreparedScene {
    pub fn map(&self) -> &GameMap {
        &self.map
    }

    /// Whether the mode posts hero switches in the chat (notices.py).
    pub fn switch_notices(&self) -> bool {
        self.switch_notices
    }

    pub fn movement_ready(&self) -> bool {
        self.contacts.is_some()
    }

    pub(crate) fn movement_index(&self) -> &TriangleIndex {
        self.ground.triangles()
    }

    /// The body's mover at the player's spawn point: the team's `number`-th,
    /// side by side when the team has fewer points (match.py Player.spawn),
    /// in the spawn rooms the match flow names (`GameMap::spawn_point_in`).
    pub(crate) fn player_spawn_mover(
        &self,
        body: u64,
        team: u8,
        number: usize,
        room: SpawnChoice,
    ) -> io::Result<Mover> {
        let (position, yaw) = self
            .map
            .spawn_point_in(team, number, room)
            .ok_or_else(|| invalid("no spawn for the selected hero"))?;
        let yaw = (yaw * 65536.0 / 360.0).round_ties_even() as i32;
        self.player_mover_at(body, team, position, yaw)
    }

    /// The body's mover with its feet at `position`, facing `yaw` (1/65536
    /// turns).
    pub(crate) fn player_mover_at(
        &self,
        body: u64,
        team: u8,
        position: [f64; 3],
        yaw: i32,
    ) -> io::Result<Mover> {
        let contacts = self.contacts.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "the scene has no native player collision",
            )
        })?;
        Mover::new(
            *self.movers.for_body(Some(body)),
            Arc::clone(contacts),
            position,
            yaw,
            mover_category(&self.map, team),
        )
        .map_err(invalid)
    }

    /// Whether feet at `position` are in one of `team`'s spawn rooms now
    /// (`map_areas::in_spawn_room`); `None` when the team has no room on
    /// the map.
    pub fn in_spawn_room(&self, team: u8, room: SpawnChoice, position: [f64; 3]) -> Option<bool> {
        map_areas::in_spawn_room(&self.spawn_rooms, &self.map, team, room, position)
    }

    /// Whether the environment kills a living body whose capsule centre is
    /// at `centre`, `above_feet` over its feet (`map_areas` module notes):
    /// in a lethal damage area of the mode or a kill volume of the map, on
    /// the catch floor of a map with neither, or in the safety net far
    /// below the whole map.
    pub fn fell_out(&self, centre: [f64; 3], above_feet: f64) -> Option<FellOut> {
        if self.lethal_areas.iter().any(|area| area.contains(centre))
            || self
                .kill_volumes
                .iter()
                .any(|volume| volume.contains(centre))
            || self
                .kill_height
                .is_some_and(|height| map_areas::caught(height, centre[1], above_feet))
        {
            Some(FellOut::Environment)
        } else if centre[1] < self.safety_net {
            Some(FellOut::BelowMap)
        } else {
            None
        }
    }

    /// The height of the map's catch floor, if it has one.
    pub fn kill_height(&self) -> Option<f64> {
        self.kill_height
    }

    /// The player body for `hero`, its scripts meeting this map
    /// ([`Self::body_map`]).
    pub(crate) fn owner_body_runtime(
        &self,
        hero: &Hero,
        spawn: OwnerBodySpawn,
    ) -> io::Result<Option<Box<dyn OwnerBodyRuntime>>> {
        self.owner_body_factory
            .as_ref()
            .map_or(Ok(None), |factory| factory(hero, spawn, self.body_map()))
    }

    /// The map as a player body's scripts meet it: its collision for their
    /// ray casts and its tagged entities for their tag queries.
    pub fn body_map(&self) -> BodyMap {
        BodyMap {
            rays: self.map_rays(),
            tags: self.map_tags(),
        }
    }

    /// The map's collision as the scripts' ray casts meet it.
    pub fn map_rays(&self) -> Arc<dyn MapRays> {
        SceneRays::shared(&self.rays, &self.ray_profiles)
    }

    /// The map's entities with target tags in this mode, for the scripts'
    /// tag queries.
    pub fn map_tags(&self) -> Arc<MapTags> {
        Arc::clone(&self.map_tags)
    }

    /// The target tags of body definition `body` (a hero's or a bot's);
    /// None when the data doesn't give them.
    pub fn body_tags(&self, body: u64) -> Option<Arc<BTreeSet<u64>>> {
        self.body_tags.get(&body).cloned()
    }

    pub fn heroes(&self) -> &HeroCatalog {
        &self.heroes
    }

    /// Each body's voice set ([`crate::voice_lines`]).
    pub fn voice_sets(&self) -> &VoiceCatalog {
        &self.voice_sets
    }

    /// The mode's respawn time for heroes (combat.py `respawn_seconds`).
    pub fn respawn_time(&self) -> Duration {
        self.respawn
    }

    /// The match's value of the mode entity's component 114 field 9, which
    /// `STU_B182C5C9` reads ([`crate::scene::mode_create`]).
    pub fn mode_random(&self) -> f32 {
        self.mode_random
    }

    /// The rules of the mode's match flow (`mode_rules_data`).
    pub fn mode_rules(&self) -> ModeRules {
        self.mode_rules
    }

    /// The map's objective data (`objective_data`).
    pub fn map_objectives(&self) -> Arc<MapObjectives> {
        Arc::clone(&self.objectives)
    }

    /// A body's mover settings.
    pub(crate) fn mover(&self, body: u64) -> &MoverSettings {
        self.movers.for_body(Some(body))
    }

    pub fn pools(&self) -> &PoolCatalog {
        &self.pools
    }

    pub fn controller(&self) -> &Controller {
        &self.controller
    }

    pub fn mode_ui(&self) -> &BitWriter {
        &self.mode_ui
    }

    /// A full frame of the game mode entity carrying the variables the
    /// match flow publishes, such as the clock v1303 (match.py
    /// `_send_countdown`, combat.py `mode_frame`).
    pub fn mode_frame(&self, variables: &Variables) -> io::Result<BitWriter> {
        mode_ui(&self.graphs, self.map.mode_guid, variables)
    }

    /// The map's server placeables in this mode with their script data, in
    /// create order.
    pub fn server_placeables(&self) -> &[Arc<PlacedScript>] {
        &self.server_placeables
    }

    /// The server placeables' scripts.
    pub fn placed_scripts(&self) -> &PlacedScripts {
        &self.placed_scripts
    }

    /// The spawn rooms the mode holds now the mode's message for, in the
    /// mode graph's place (`choice`: each team's current spawn, the match
    /// flow's; [`map_areas::room_holds`]).
    pub fn room_holds(&self, choice: impl Fn(u8) -> SpawnChoice) -> Vec<(u32, Option<u8>)> {
        map_areas::room_holds(&self.spawn_rooms, &self.map, choice)
    }

    /// Start the server placeables' scripts at the first call, give the
    /// spawn rooms `holds` (none where the mode graph sends its `0724`
    /// itself; else [`Self::room_holds`]) the mode's message, then advance
    /// them to `tick`.
    pub fn advance_placed(&mut self, tick: u32, holds: &[(u32, Option<u8>)]) {
        self.placed_scripts.advance(tick, holds);
    }

    /// The placed scripts' game messages and message ends since the last
    /// call, for the match to route ([`crate::messages`]).
    pub fn take_placed_messages(&mut self) -> (Vec<GameMessage>, Vec<MessageEnd>) {
        self.placed_scripts.take_messages()
    }

    /// The player and first body the next dummy bot the placed scripts
    /// make gets ([`PlacedScripts::set_next_dummy`]).
    pub fn set_next_dummy(&mut self, next: Option<NextDummy>) {
        self.placed_scripts.set_next_dummy(next);
    }

    /// The dummy bots the placed scripts made or removed since the last
    /// call, for the match to make or remove.
    pub fn take_dummy_changes(&mut self) -> Vec<DummyChange> {
        self.placed_scripts.take_dummy_changes()
    }

    /// The scene's entities that take game messages at world tick `tick`
    /// ([`crate::messages`]): the Training Bots and the placed scripts.
    pub fn message_recipients(&mut self, tick: u32) -> (Bots<'_>, &mut PlacedScripts) {
        (
            Bots {
                scripts: &mut self.bot_scripts,
                tick,
            },
            &mut self.placed_scripts,
        )
    }

    /// The bodies the placeables' target searches can find from now on
    /// (`Combat::target_candidates`).
    pub fn set_placed_target_candidates(&mut self, candidates: &[TargetCandidate]) {
        self.placed_scripts.set_target_candidates(candidates);
    }

    /// The statescript graphs, for runs outside the scene (the mode graph).
    pub fn shared_graphs(&self) -> Arc<GraphStore> {
        Arc::clone(&self.graphs)
    }

    /// The statescript graphs and the data loaded with them.
    pub fn graphs(&self) -> &GraphStore {
        &self.graphs
    }

    /// The mode graphs' values the match flow uses (the wait after a
    /// round, the overtime fuse, Control's capture times).
    pub fn mode_graph_values(&self) -> ModeGraphValues {
        self.mode_graph_values
    }

    pub fn bots(&self) -> &[Bot] {
        &self.bots
    }

    pub fn bots_mut(&mut self) -> &mut [Bot] {
        &mut self.bots
    }

    /// The living training bots' capsules where they are now, with their
    /// teams (`Bot::capsule`), for the bodies they block
    /// (`replication::blocks`).
    pub fn bot_capsules(&self) -> impl Iterator<Item = (u8, CapsulePose)> + '_ {
        self.bots.iter().filter_map(|bot| {
            let capsule = bot.capsule(self.mover(bot.spawn.body))?;
            Some((bot.spawn.team, capsule))
        })
    }

    /// A bot's body as a target: the capsule of its mover's standing height
    /// and radius (combat.py `_targets`), facing its yaw as the clients see
    /// it, hit by its body's shapes when the data has them: posed this tick
    /// when it has a rig ([`crate::bot_animation`]), else the static ones.
    pub(crate) fn bot_target(&self, bot: &Bot) -> crate::hitscan::TargetPose {
        let settings = self.movers.for_body(Some(bot.spawn.body));
        crate::hitscan::TargetPose {
            feet: bot.position,
            height: settings.stand_height,
            radius: settings.stand_radius,
            eye: crate::bots::BOT_EYE,
            yaw: bot.facing(),
            shapes: bot
                .posed_shapes()
                .or_else(|| self.hitboxes.for_body(bot.spawn.body)),
        }
    }

    /// The Tab board catalog (stats.py).
    pub fn stats(&self) -> &StatsCatalog {
        &self.stats
    }

    /// The stats the server counts for each player.
    pub fn board_stats(&self) -> &BoardStats {
        &self.board
    }

    /// The Biotic Field's initial graphs (`crate::spawned`).
    pub(crate) fn field_graphs(&self) -> &[u32] {
        &self.field_graphs
    }

    /// A player's body as a target (combat.py `_targets`): a capsule from
    /// the feet, crouched or standing, with the mover's standing radius. It
    /// keeps the capsule even where the data has the hero's hit shapes: they
    /// are a rest pose, which cannot follow a crouch or an animation.
    pub(crate) fn player_target(
        &self,
        hero: &Hero,
        feet: [f64; 3],
        crouched: bool,
    ) -> crate::hitscan::TargetPose {
        let settings = self.movers.for_body(Some(hero.body));
        crate::hitscan::TargetPose {
            feet,
            height: if crouched {
                settings.crouch_height
            } else {
                settings.stand_height
            },
            radius: settings.stand_radius,
            eye: self.eyes.for_body(hero.body).height(crouched),
            yaw: 0.0,
            shapes: None,
        }
    }

    /// How far along a shot or a projectile's sweep the map's collision
    /// first stops it: the ray cache's shapes whose masks share a bit with
    /// the map mask of the volley's ray profile (`profile`, its
    /// `m_C1903771`; [`RayProfiles::map_mask`]). The movement hull alone
    /// let shots through every shape the mover doesn't meet, the visual
    /// detail (refs report `WeaponRays.md`; Python queried the movement
    /// geometry, combat.py `ray_world`).
    pub(crate) fn hitscan_wall(
        &self,
        origin: [f64; 3],
        direction: [f64; 3],
        reach: f64,
        profile: Option<u64>,
    ) -> Result<Option<f64>, orpheus_physics::collision::index::IndexError> {
        self.rays
            .cast_ray(
                origin,
                direction,
                reach,
                self.ray_profiles.map_mask(profile),
            )
            .map(|hit| hit.map(|hit| hit.distance))
    }

    /// The map's AI navigation, if the data has it.
    pub fn navigation(&self) -> Option<&Navigation> {
        self.navigation.as_deref()
    }

    /// Whether nothing of the map's collision stands between two points,
    /// as a hitscan shot from one to the other sees it (an AI's line of
    /// sight): a shot of a volley that names no ray profile, as most
    /// heroes' main weapons do.
    pub fn line_of_sight(&self, from: [f64; 3], to: [f64; 3]) -> bool {
        let delta = [0, 1, 2].map(|axis| to[axis] - from[axis]);
        let reach = (delta[0].powi(2) + delta[1].powi(2) + delta[2].powi(2)).sqrt();
        if reach < 1e-6 {
            return true;
        }
        let direction = delta.map(|value| value / reach);
        matches!(self.hitscan_wall(from, direction, reach, None), Ok(None))
    }

    /// One more bot gets its script (match.py `update`: one per update,
    /// within the loading window), then the motions the scripts began last
    /// tick (a velocity action, `STU_FF3471E9`) put their bots on their
    /// movers ([`Bot::start_motions`]), the knocked or moved bots fly
    /// ([`Bot::fly`]), the others walk to the point their scripts ask for
    /// (the patrol `0B8E`; a walk that ended goes back to the script, which
    /// takes it when it next runs), and the live ones are posed
    /// ([`crate::bot_animation`]), by what their scripts asked as of their
    /// last run.
    pub fn step_bots(&mut self, tick: u32) {
        self.bot_scripts
            .prepare_next(&self.bots, tick, &self.bot_plate);
        let floor = self.ground.triangles();
        for (slot, bot) in self.bots.iter_mut().enumerate() {
            let motions = self.bot_scripts.take_motions(slot, bot.entity);
            if !motions.is_empty() {
                let mover = bot_mover(self.contacts.as_ref(), &self.movers, &self.map, bot, tick);
                bot.start_motions(motions, tick.saturating_sub(1), mover);
            }
            bot.fly(tick, floor);
            let goal = self.bot_scripts.move_goal(slot, bot.entity);
            let ended = bot.step(
                tick,
                goal.map(|goal| goal.target),
                ground_height(floor, mover_category(&self.map, bot.spawn.team)),
            );
            if let (Some(goal), Some(ended)) = (goal, ended) {
                self.bot_scripts.move_arrived(slot, bot.entity, goal, ended);
            }
            if !bot.dead {
                let (arms_forward, anim) = self
                    .bot_scripts
                    .snapshot(slot, bot.entity)
                    .map_or((false, None), |snapshot| {
                        self.script_anims.requests(snapshot)
                    });
                bot.animate(tick, arms_forward, anim);
            }
        }
    }

    /// A knockback on a live bot (an explosion's push,
    /// [`crate::projectile_flight::ProjectileFlight::blast_push`]): it flies
    /// on its body's mover from its next frame on (`tick` is this one), as
    /// a velocity action moves it ([`Self::step_bots`]); `0E8C`'s knockback
    /// anim waits for the ground (#545), so the knockback moves them.
    /// Assumption: nothing in the data spares them. Nothing happens without
    /// player collision.
    pub(crate) fn knock_bot(&mut self, entity: u32, push: [f64; 3], tick: u32) {
        let Some(bot) = self.bots.iter_mut().find(|bot| bot.entity == entity) else {
            return;
        };
        let mover = bot_mover(self.contacts.as_ref(), &self.movers, &self.map, bot, tick);
        bot.knock(push, tick, mover);
    }

    pub fn bot_scripts(&self) -> &BotScripts {
        &self.bot_scripts
    }

    /// The world waits until every bot that has a script has started it
    /// (match.py `_spawn_steps`: `not self._pending_bot_scripts`).
    pub fn bot_scripts_ready(&self) -> bool {
        self.bot_scripts.ready()
    }

    /// The firing bots' weapon scripts at this world tick; their shots for
    /// the match to fly (combat.py `_bot_weapons`).
    pub fn bot_weapons(&mut self, tick: u32) -> Vec<BotShot> {
        self.bot_scripts.weapons(&self.bots, tick)
    }

    /// The bodies the bots' target searches can find from now on
    /// (`Combat::target_candidates`).
    pub fn set_bot_target_candidates(&mut self, candidates: &[TargetCandidate]) {
        self.bot_scripts.set_target_candidates(candidates);
    }

    /// A bot was killed: its script's death message (combat.py `_bot_died`).
    pub fn bot_died(&mut self, death: &BotDeath) {
        self.bot_scripts.died(death);
    }

    /// A bot's pools after a hit it survived or a heal (combat.py
    /// `_sync_body_health`).
    pub fn bot_health(&mut self, entity: u32, health: f64) {
        self.bot_scripts.set_health(entity, health);
    }

    /// A hit the bot survived reaches its script as `0011.025` (its flinch,
    /// then its pain line), after [`Self::bot_health`].
    pub fn bot_hurt(&mut self, entity: u32, damage: &crate::combat::BotDamage) {
        self.bot_scripts.hurt(entity, damage, &self.bots);
    }

    /// An aura comes on the live bot at world tick `tick` or goes off it
    /// (Biotic Grenade's explosion, an ApplyAura).
    pub fn change_bot_aura(&mut self, change: &orpheus_script::vm::AuraChange, tick: u32) {
        self.bot_scripts.change_aura(change, tick);
    }

    /// The aura changes the bots' graphs made since the last call.
    pub fn take_bot_aura_changes(&mut self) -> Vec<orpheus_script::vm::AuraChange> {
        self.bot_scripts.take_aura_changes()
    }

    /// The health changes the bots' graphs made since the last call.
    pub fn take_bot_health_changes(&mut self) -> Vec<orpheus_script::vm::HealthChange> {
        self.bot_scripts.take_health_changes()
    }

    /// The dead bots' death scripts (combat.py `_advance_bot_death`).
    pub fn advance_bot_deaths(&mut self, tick: u32) {
        self.bot_scripts.advance_deaths(&self.bots, tick);
    }

    /// The live bots' graph events due by this tick (a hit's reaction), and
    /// the voice lines their criteria call for.
    pub fn advance_live_bots(&mut self, tick: u32) {
        self.bot_scripts.advance_live(&self.bots, tick);
    }

    /// The voice lines the bots said since the last call, each with where
    /// its bot stood, for the clients that hear it
    /// ([`crate::voice_lines::VoiceLine::heard`]).
    pub fn take_bot_voices(&mut self) -> Vec<(crate::voice_lines::VoiceLine, [f64; 3])> {
        self.bot_scripts.take_voices()
    }

    /// The respawned bots' new scripts (combat.py `_respawn_bot`).
    pub fn prepare_respawned_bots(&mut self, tick: u32) {
        self.bot_scripts
            .prepare_respawned(&self.bots, tick, &self.bot_plate);
    }

    /// The live nameplate tool's changes in force (`bot_plate`).
    pub fn bot_plate(&self) -> &Arc<BotPlate> {
        &self.bot_plate
    }

    /// The live hurt feedback switch in force (`hurt_feedback`).
    pub fn hurt_feedback(&self) -> HurtFeedback {
        self.hurt
    }

    pub fn set_hurt_feedback(&mut self, hurt: HurtFeedback) {
        self.hurt = hurt;
    }

    /// The live nameplate tool's changes from now on: the live bots'
    /// scripts take its variables at once (their changed frames come with
    /// the next bot events), and each live bot's pools update comes back,
    /// for every client of the room. `vitals` gives a bot's pools by slot
    /// (the match's combat; none: it has its full health).
    pub fn set_bot_plate<'v>(
        &mut self,
        plate: Arc<BotPlate>,
        vitals: impl Fn(usize) -> Option<&'v Vitals>,
    ) -> Vec<EntityUpdate> {
        let previous = std::mem::replace(&mut self.bot_plate, plate);
        if previous == self.bot_plate {
            return Vec::new();
        }
        self.bot_scripts.apply_plate(&self.bots, &self.bot_plate);
        self.bots
            .iter()
            .enumerate()
            .filter(|(_, bot)| !bot.dead)
            .filter_map(|(slot, bot)| {
                let whole;
                let vitals = match vitals(slot) {
                    Some(vitals) => vitals,
                    None => {
                        whole = Vitals::new(bot.spawn.health, 0.0, 0.0);
                        &whole
                    }
                };
                let pools = self.bot_plate.pool_update(&previous, vitals)?;
                Some(EntityUpdate {
                    entity: bot.entity,
                    operation: Some(Operation::Update(vec![pools])),
                    ..EntityUpdate::default()
                })
            })
            .collect()
    }

    /// The live nameplate tool's game message to every live bot's
    /// nameplates; how many took it.
    pub fn message_bot_plates(&mut self, message: u64) -> usize {
        self.bot_scripts.message_plates(&self.bots, message)
    }

    /// The first live bot with a script as the live nameplate tool shows
    /// it; `vitals` as for [`Self::set_bot_plate`].
    pub fn bot_plate_sample<'v>(
        &self,
        vitals: impl Fn(usize) -> Option<&'v Vitals>,
    ) -> Option<PlateSample> {
        self.bots
            .iter()
            .enumerate()
            .filter(|(_, bot)| !bot.dead)
            .find_map(|(slot, bot)| {
                let variables = self
                    .bot_scripts
                    .plate_variables(bot.entity, &self.bot_plate)?;
                let whole;
                let vitals = match vitals(slot) {
                    Some(vitals) => vitals,
                    None => {
                        whole = Vitals::new(bot.spawn.health, 0.0, 0.0);
                        &whole
                    }
                };
                Some(PlateSample {
                    entity: bot.entity,
                    name: bot.spawn.name.clone(),
                    parts: self.bot_plate.parts(vitals),
                    variables,
                })
            })
    }

    /// What the bot scripts produced for the clients since the last call.
    pub fn take_bot_events(&mut self) -> Vec<BotEvent> {
        self.bot_scripts.take_events()
    }

    /// The world a client gets as it starts. A bot whose states hold target
    /// tags carries them in its create as component 76: the client's
    /// create (`0x7FF78969C360`) reads the create's components with the
    /// update reader (`0x7FF78969E5D0`) once the entity's components are
    /// built (`refs/reports/AllyBars.md` "NightBotMotion").
    pub fn initial_world(&self, viewer: &PlayerAppearance<'_>) -> io::Result<Vec<EntityUpdate>> {
        let mut updates = initial_world(
            &self.map,
            viewer,
            self.initial_board.clone(),
            &self.bots,
            &self.bot_plate,
            self.mode_random,
        )?;
        for update in &mut updates {
            if let (Some((_, tags)), Some(Operation::Create(definition))) =
                (self.bot_scripts.tags(update.entity), &mut update.operation)
            {
                definition.components.push(tags.clone());
            }
        }
        // After the placeables, the player and the mode, before the bots.
        let at = self.map.placeables.len() + 2;
        updates.splice(
            at..at,
            self.server_placeables.iter().map(|placed| EntityUpdate {
                entity: placed.entity(),
                operation: Some(Operation::Placeable(
                    placed.placeable,
                    placeable_components(placed),
                )),
                ..EntityUpdate::default()
            }),
        );
        Ok(updates)
    }
}

#[derive(Deserialize)]
struct PropCatalog {
    modes: BTreeMap<String, Vec<String>>,
}

/// The Biotic Field's initial graphs: those of [`FIELD_ENTITY`] in
/// `game_spawns_174.json` (`0258`). Assumption: they bind no values, as
/// the file lists none.
fn field_graphs(path: &Path) -> io::Result<Vec<u32>> {
    #[derive(Deserialize)]
    struct Spawn {
        graphs: Vec<u32>,
    }
    let spawns: HashMap<String, Spawn> =
        serde_json::from_str(&fs::read_to_string(path)?).map_err(invalid)?;
    spawns
        .get(&format!("0x{FIELD_ENTITY:016X}"))
        .map(|spawn| spawn.graphs.clone())
        .filter(|graphs| !graphs.is_empty())
        .ok_or_else(|| invalid("the Biotic Field entity starts no graph"))
}

fn read_mode_props(path: &Path) -> io::Result<BTreeMap<u64, BTreeSet<u64>>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error),
    };
    let catalog: PropCatalog = serde_json::from_slice(&bytes).map_err(invalid)?;
    let guid = |value: &str| parse_guid(value).ok_or_else(|| invalid("invalid mode prop GUID"));
    catalog
        .modes
        .into_iter()
        .map(|(mode, props)| {
            Ok((
                guid(&mode)?,
                props
                    .iter()
                    .map(|value| guid(value))
                    .collect::<io::Result<_>>()?,
            ))
        })
        .collect()
}

/// Body definition GUID → its target tags.
type BodyTags = HashMap<u64, Arc<BTreeSet<u64>>>;

/// The target tags of every body definition the bundles give them for: the
/// heroes' (hero bundle) and the bots' (bot bundle).
fn body_tags(graphs: &GraphStore, bots: Option<&GraphStore>) -> BodyTags {
    let heroes = graphs.bodies().iter();
    let bots = bots.into_iter().flat_map(|bots| bots.entity_bodies());
    heroes
        .chain(bots)
        .filter_map(|(guid, body)| Some((*guid, Arc::new(body.target_tags.clone()?))))
        .collect()
}

/// The version of a collision cache file, `None` when it is none
/// (collision.py `cache_version`).
fn cache_version(path: &Path) -> Option<u32> {
    let mut head = [0; 12];
    File::open(path).ok()?.read_exact(&mut head).ok()?;
    (&head[..8] == MAGIC).then(|| u32::from_le_bytes([head[8], head[9], head[10], head[11]]))
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

/// Builds a map's collision caches from `install`, for tests that prepare
/// a map the runtime pack has no caches for.
#[cfg(test)]
pub(crate) fn install_collision_build(install: PathBuf) -> Arc<CollisionBuild> {
    Arc::new(move |map, name| {
        let opened = orpheus_assets::Install::open(&install, "enUS").map_err(io::Error::other)?;
        Ok(CollisionFiles {
            movement: orpheus_assets::collision_cache(&opened, map, name)
                .map_err(io::Error::other)?
                .bytes,
            rays: orpheus_assets::ray_cache(&opened, map, name)
                .map_err(io::Error::other)?
                .bytes,
        })
    })
}

#[cfg(test)]
mod ported_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bots::BOT_ENTITY, instance::RETAIL_QUANTA, scene::team_bit};
    use orpheus_content::heroes::SOLDIER;
    use orpheus_core::tick::FIRST_TICK;

    /// The mode graph's tag queries find the map's tagged placeables: on
    /// Hanamura's Assault, `0F4A`'s child `0C8F` lists each team's spawn
    /// points (#8365, #8366), which its spawn picker draws from. Until
    /// `0F4A` writes the teams' spawn tags (#8364e, from the objectives'
    /// #10551, which this world never gets) a query names no any-of tag,
    /// so both lists are every placeable with #11016's tag (`042C`).
    /// `0F4A`'s #12750 (tag `04B3`) is answered, and empty: no placeable of
    /// any map carries `04B3`.
    #[test]
    #[ignore = "requires a derived 104319 pack (just test-game <install>)"]
    fn the_mode_graph_lists_hanamuras_spawn_points() {
        use orpheus_content::maps::MapCatalog;
        use orpheus_script::{
            mode::{ModePlayer, ModeScript, ModeWorld},
            wire::Value,
        };
        let runtime = orpheus_testkit::runtime_dir();
        let data = runtime.join("data");
        let assets = SceneAssets::load(&runtime).unwrap();
        let catalog = MapCatalog::load(&data)
            .unwrap()
            .with_modes(GameModes::load(data.join(MODES_FILE)).unwrap());
        let mode = crate::match_flow::ASSAULT;
        let hanamura = catalog
            .map_catalog()
            .into_iter()
            .find(|listing| listing.name == "Hanamura")
            .unwrap();
        let map = catalog
            .game_map(hanamura.guid, mode, "Assault", false, &[6, 6])
            .unwrap();
        let empty = BTreeSet::new();
        let identifiers = assets.mode_props.get(&mode).unwrap_or(&empty);
        let mut world = ModeWorld::new(Arc::clone(&assets.graphs), mode, Unknowns::Tolerate);
        world.players = (0..2)
            .map(|team| ModePlayer {
                controller: 0x1001 + team as u32,
                body: None,
                team,
            })
            .collect();
        world.team_sizes = vec![(0, 6), (1, 6)];
        world.ready = true;
        let tags = assets.map_areas.map_tags(&map, identifiers, &assets.graphs);
        world.map_tags = Some(Arc::clone(&tags));
        let mut script = ModeScript::start(world, 0x0f4a, 1).unwrap();
        let entities = |value: Option<&Value>| match value {
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| match item {
                    Value::Entity(entity) => *entity,
                    other => panic!("{other:?}"),
                })
                .collect(),
            _ => Vec::new(),
        };
        let mut spawns = [Vec::new(), Vec::new()];
        for frame in 2..3000 {
            script.advance(frame).unwrap();
            let snapshot = script.snapshot().unwrap();
            if let Some(picker) = snapshot.instances.values().find(|i| i.graph == 0x0c8f) {
                spawns = [8365, 8366].map(|var| entities(picker.variables.get(&var)));
                if spawns.iter().all(|list| !list.is_empty()) {
                    break;
                }
            }
        }
        let snapshot = script.snapshot().unwrap();
        assert_eq!(
            snapshot.instances[&1].variables.get(&12750),
            Some(&Value::Array(Vec::new()))
        );
        assert_eq!(snapshot.variables.get(&8364), None);
        let every = tags.placed_with_tag(0x0d60_0000_0000_042c);
        assert!(!every.is_empty());
        assert_eq!(spawns, [every.clone(), every]);
    }

    #[test]
    #[ignore = "requires a derived 104319 Practice Range pack without bot VM graphs"]
    fn stock_heroes_attach_their_authored_owner_scripts() {
        let runtime = orpheus_testkit::runtime_dir();
        let assets = SceneAssets::load(runtime).unwrap();
        let scene = assets.prepare(GameMap::practice_range()).unwrap();
        assert!(scene.movement_ready());
        // Not only Soldier: 76 (`stock_soldier_plays_a_scripted_session`) has
        // an owner script: Pharah's starts with no unknown content.
        let pharah = scene.heroes().find_hero("Pharah").unwrap();
        let spawn = OwnerBodySpawn {
            slot: 0,
            controller: 0xa000_0100,
            entity: 0xa000_0101,
            body_filter: team_bit(0).unwrap(),
            frame: FIRST_TICK,
            pose: scene
                .player_spawn_mover(pharah.body, 0, 0, SpawnChoice::First)
                .unwrap()
                .pose(),
            quanta: RETAIL_QUANTA,
            mode_random: scene.mode_random(),
            ai: None,
        };
        assert!(
            scene.owner_body_runtime(pharah, spawn).unwrap().is_some(),
            "the stock Pharah owner script did not start"
        );
    }

    /// The client builds a gun only when its definition is preloaded, and the
    /// captured preload list has none, so the guns come from the weapon
    /// scripts. Five weapon scripts build no cosmetic (AipNooBest's table
    /// agrees); those heroes' arms or bodies are the weapon.
    #[test]
    #[ignore = "requires a derived 104319 Practice Range pack without bot VM graphs"]
    fn the_weapon_scripts_name_the_guns_to_preload() {
        let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
        let graphs = assets.graphs();
        let guns = graphs.weapon_cosmetics();
        assert!(guns.contains(&0x0400_0000_0000_04bf), "Soldier: 76's rifle");
        let gunless: Vec<_> = graphs
            .bodies()
            .values()
            .filter(|body| body.hero.is_some() && body.weapons.iter().any(Option::is_some))
            .filter(|body| graphs.body_weapon_cosmetics(body).next().is_none())
            .map(|body| body.name.as_str())
            .collect();
        assert_eq!(
            gunless,
            ["Bastion", "Doomfist", "Moira", "Wrecking Ball", "Echo"]
        );
    }

    /// Soldier: 76 through the server's own path, frame by frame: empty the
    /// rifle into its reload, then move, sprint, crouch, Helix and melee. Every
    /// frame must run without a gap and encode for the owner and observers.
    #[test]
    #[ignore = "requires a derived 104319 Practice Range pack without bot VM graphs"]
    fn stock_soldier_plays_a_scripted_session() {
        use crate::replication::BodyStep;
        use orpheus_script::snapshot::View;
        use orpheus_wire::commands::Command;

        const PRIMARY: u32 = 0x1;
        const SECONDARY: u32 = 0x2;
        const SPRINT: u32 = 0x8;
        const CROUCH: u32 = 0x80;
        const MELEE: u32 = 0x800;

        let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
        let scene = assets.prepare(GameMap::practice_range()).unwrap();
        let hero = scene.heroes().get(SOLDIER).unwrap();
        let mover = scene
            .player_spawn_mover(hero.body, 0, 0, SpawnChoice::First)
            .unwrap();
        let spawn = OwnerBodySpawn {
            slot: 0,
            controller: 0xa000_0100,
            entity: 0xa000_0101,
            body_filter: team_bit(0).unwrap(),
            frame: FIRST_TICK,
            pose: mover.pose(),
            quanta: RETAIL_QUANTA,
            mode_random: scene.mode_random(),
            ai: None,
        };
        let mut runtime = scene.owner_body_runtime(hero, spawn).unwrap().unwrap();
        // (frames, forward throttle, buttons held)
        let script: [(u32, i8, u32); 10] = [
            (30, 0, 0),
            (300, 0, PRIMARY),
            (200, 0, 0),
            (60, 127, 0),
            (60, 127, SPRINT),
            (30, 0, CROUCH),
            (30, 0, 0),
            (2, 0, SECONDARY),
            (60, 0, 0),
            (2, 0, MELEE),
        ];
        let mut frame = spawn.frame;
        let mut shots = 0;
        for (frames, forward, buttons) in script {
            for _ in 0..frames {
                frame += 1;
                let command = Command {
                    frame,
                    forward,
                    buttons,
                    ..Command::default()
                };
                runtime.advance(command, &mover, BodyStep::Posed).unwrap();
                shots += runtime.take_shots(Some(command), &mover).len();
                let snapshot = runtime.snapshot().unwrap();
                assert!(
                    snapshot.diagnostics.is_empty(),
                    "frame {frame}: {:?}",
                    snapshot.diagnostics
                );
                let encoder = runtime.encoder();
                for view in [View::Owner, View::Observer] {
                    let projected = encoder.project(snapshot, view).unwrap();
                    encoder.full(&projected).unwrap();
                }
            }
        }
        // Every shot of the session, Helix included; a change here means
        // the weapons' timing changed. Quick melee is no shot: its graph
        // (`0043`) strikes only the bodies its search finds, and this world
        // lists none.
        assert_eq!(shots, 31);
        let snapshot = runtime.snapshot().unwrap();
        let assumed: Vec<_> = snapshot
            .assumptions
            .iter()
            .map(|item| {
                (
                    item.site.graph,
                    item.site.node,
                    item.class.as_str(),
                    item.answer,
                )
            })
            .collect();
        // The answers the server gives without knowing they are right, as the
        // Python reference did (docs/notes/behavior.md). Each one is a known
        // difference to check with the client.
        assert_eq!(
            assumed,
            [
                (0x01ca, Some(126), "STU_ED45C5E8", "no entities"),
                (
                    0x02c5,
                    Some(51),
                    "STUStatescriptStateAnim",
                    "never ends by itself"
                ),
                (0x0255, Some(40), "STU_E9694EFD", "no-op"),
                (
                    0x0043,
                    Some(11),
                    "STUStatescriptStateTrackTargets bodies",
                    "finds no targets (the world lists none)"
                ),
                (0x0043, Some(9), "STU_23A00A53", "no effect"),
                (
                    0x0043,
                    Some(10),
                    "STUStatescriptStateModifyHealth m_B12BFE8B",
                    "not read"
                ),
                (
                    0x0043,
                    Some(10),
                    "STUStatescriptModifyHealth m_knockbackRings",
                    "not read"
                ),
                (
                    0x0043,
                    Some(10),
                    "STUStatescriptModifyHealth m_C0139FA6",
                    "not read"
                ),
                (
                    0x0043,
                    Some(10),
                    "STUStatescriptModifyHealth m_E33EFEE1",
                    "not read"
                ),
                (
                    0x0043,
                    Some(10),
                    "STUStatescriptModifyHealth m_10399AE7",
                    "not read"
                ),
                (0x0043, Some(63), "STU_0A6BE10E", "no effect"),
                (
                    0x0043,
                    Some(43),
                    "STUStatescriptStatePhysicsExplosion",
                    "no effect"
                ),
                (0x0043, Some(38), "STU_54FEB21C", "no effect"),
                (0x0043, Some(24), "STU_B5C87F3D", "no effect"),
                (0x0043, Some(60), "STU_77E13026", "no hits"),
            ],
            "{} assumptions",
            assumed.len()
        );
        // The one typed-pair value (05AD's message table) encodes with the
        // rest of the frames; no expression map is left.
        let tables: Vec<_> = snapshot
            .instances
            .values()
            .flat_map(|instance| {
                instance
                    .variables
                    .iter()
                    .filter(|(_, value)| {
                        matches!(
                            value,
                            orpheus_script::wire::Value::Map(_)
                                | orpheus_script::wire::Value::Pairs(_)
                        )
                    })
                    .map(|(&variable, _)| (instance.graph, variable))
            })
            .collect();
        assert_eq!(tables, [(0x05ad, 29859)]);
    }

    /// Soldier: 76's Biotic Field (ABILITY_2, graph `0257`): 0.65 s after
    /// the cast its target search (st6, node 11) counts the living allies
    /// within 10 m of a point a metre over his feet, himself left out, into
    /// #1192, and the transition plug ends it at once (its criteria is the
    /// client's). An allied bot 3 m away counts; the caster, an enemy bot
    /// 2 m away and an ally 30 m away don't. Every frame encodes for the
    /// owner and observers.
    #[test]
    #[ignore = "requires a derived 104319 Practice Range pack without bot VM graphs"]
    fn the_biotic_field_counts_the_allies_near_its_caster() {
        use crate::replication::BodyStep;
        use orpheus_script::{
            snapshot::View,
            vm::{StateKind, TargetCandidate},
            wire::Value,
        };
        use orpheus_wire::commands::Command;

        /// tests/test_game_projectiles.py `ABILITY_2`: 0257 st0's button.
        const ABILITY_2: u32 = 0x10;
        const FIELD_COUNT: u32 = 1192;
        let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
        let scene = assets.prepare(GameMap::practice_range()).unwrap();
        let hero = scene.heroes().get(SOLDIER).unwrap();
        let mover = scene
            .player_spawn_mover(hero.body, 0, 0, SpawnChoice::First)
            .unwrap();
        let spawn = OwnerBodySpawn {
            slot: 0,
            controller: 0xa000_0100,
            entity: 0xa000_0101,
            body_filter: team_bit(0).unwrap(),
            frame: FIRST_TICK,
            pose: mover.pose(),
            quanta: RETAIL_QUANTA,
            mode_random: scene.mode_random(),
            ai: None,
        };
        let [x, y, z] = spawn.pose.position;
        let body = |entity, team, dx: f64| TargetCandidate {
            entity,
            team: Some(team),
            dead: false,
            controller: None,
            ai: false,
            ai_control: None,
            feet: [x + dx, y, z],
            height: 1.8,
            radius: 0.4,
            yaw: 0.0,
            tags: None,
            spawned: Vec::new(),
        };
        let count = |candidates: &[TargetCandidate]| {
            let mut runtime = scene.owner_body_runtime(hero, spawn).unwrap().unwrap();
            runtime.set_target_candidates(candidates);
            let buttons = std::iter::repeat_n(0, 30)
                .chain([ABILITY_2])
                .chain(std::iter::repeat_n(0, 120));
            for (frame, buttons) in (spawn.frame + 1..).zip(buttons) {
                let command = Command {
                    frame,
                    buttons,
                    ..Command::default()
                };
                runtime.advance(command, &mover, BodyStep::Posed).unwrap();
                runtime.take_shots(Some(command), &mover);
                let snapshot = runtime.snapshot().unwrap();
                assert!(
                    snapshot.diagnostics.is_empty(),
                    "{:?}",
                    snapshot.diagnostics
                );
                let encoder = runtime.encoder();
                for view in [View::Owner, View::Observer] {
                    encoder
                        .full(&encoder.project(snapshot, view).unwrap())
                        .unwrap();
                }
            }
            let snapshot = runtime.snapshot().unwrap();
            let field = snapshot
                .instances
                .values()
                .find(|instance| instance.graph == 0x0257)
                .unwrap();
            let search = &field.states[&6];
            assert_eq!(search.kind, StateKind::TrackTargets);
            assert_eq!((search.active, search.activation), (false, 1));
            // The client's line of sight (`m_95FFC3C5`) and rewind
            // (`m_1C2A9945`) are what the search runs without.
            assert!(snapshot.assumptions.iter().any(|item| {
                (
                    item.site.graph,
                    item.site.node,
                    item.class.as_str(),
                    item.answer,
                ) == (
                    0x0257,
                    Some(11),
                    "STUStatescriptStateTrackTargets m_95FFC3C5",
                    "no line-of-sight test",
                )
            }));
            field.variables.get(&FIELD_COUNT).cloned()
        };
        let caster = body(spawn.entity, 0, 0.0);
        let enemy = body(BOT_ENTITY, 1, 2.0);
        let far = body(BOT_ENTITY + 1, 0, 30.0);
        let ally = body(BOT_ENTITY + 2, 0, 3.0);
        assert_eq!(
            count(&[caster.clone(), enemy.clone(), far.clone()]),
            Some(Value::Integer(0))
        );
        assert_eq!(count(&[caster, enemy, far, ally]), Some(Value::Integer(1)));
    }

    /// Soldier: 76's quick melee (`0043`) as the server runs it, looking
    /// down an eighth of a turn: its search's sphere, 1.5 m along the aim
    /// from his eye, reaches his own capsule and an ally's beside him, yet
    /// a swing at nothing changes no health; a swing with an enemy bot in
    /// front strikes the bot alone, once, for 30 from the swinger.
    #[test]
    #[ignore = "requires a derived 104319 Practice Range pack without bot VM graphs"]
    fn quick_melee_strikes_the_enemy_in_front_never_the_swinger() {
        use crate::{combat::PosedShot, replication::BodyStep};
        use orpheus_script::vm::TargetCandidate;
        use orpheus_wire::commands::Command;

        const MELEE: u32 = 0x800;
        /// An eighth of a turn down.
        const PITCH: i32 = 8192;
        let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
        let scene = assets.prepare(GameMap::practice_range()).unwrap();
        let hero = scene.heroes().get(SOLDIER).unwrap();
        let mover = scene
            .player_spawn_mover(hero.body, 0, 0, SpawnChoice::First)
            .unwrap();
        let spawn = OwnerBodySpawn {
            slot: 0,
            controller: 0xa000_0100,
            entity: 0xa000_0101,
            body_filter: team_bit(0).unwrap(),
            frame: FIRST_TICK,
            pose: mover.pose(),
            quanta: RETAIL_QUANTA,
            mode_random: scene.mode_random(),
            ai: None,
        };
        let [x, y, z] = spawn.pose.position;
        // Yaw 0 looks along +z.
        let body = |entity, team, [dx, dz]: [f64; 2]| TargetCandidate {
            entity,
            team: Some(team),
            dead: false,
            controller: None,
            ai: false,
            ai_control: None,
            feet: [x + dx, y, z + dz],
            height: 1.8,
            radius: 0.4,
            yaw: 0.0,
            tags: None,
            spawned: Vec::new(),
        };
        let swing = |candidates: &[TargetCandidate]| {
            let mut runtime = scene.owner_body_runtime(hero, spawn).unwrap().unwrap();
            runtime.set_target_candidates(candidates);
            let buttons = std::iter::repeat_n(0, 30)
                .chain(std::iter::repeat_n(MELEE, 2))
                .chain(std::iter::repeat_n(0, 90));
            let mut struck = Vec::new();
            for (frame, buttons) in (spawn.frame + 1..).zip(buttons) {
                let command = Command {
                    frame,
                    buttons,
                    pitch: PITCH,
                    ..Command::default()
                };
                runtime.advance(command, &mover, BodyStep::Posed).unwrap();
                for shot in runtime.take_shots(Some(command), &mover) {
                    if let PosedShot::Health(change) = shot {
                        struck.push((change.target, change.instigator, change.amount, change.heal));
                    }
                }
                let snapshot = runtime.snapshot().unwrap();
                assert!(
                    snapshot.diagnostics.is_empty(),
                    "{:?}",
                    snapshot.diagnostics
                );
            }
            let snapshot = runtime.snapshot().unwrap();
            let melee = snapshot
                .instances
                .values()
                .find(|instance| instance.graph == 0x0043)
                .unwrap();
            assert_eq!(melee.states[&5].activation, 1, "the swing searched once");
            struck
        };
        // Bodies on all four sides, so one stands where he looks whichever
        // way the spawn turns him.
        let around = |first: u32, team, reach: f64| {
            [[0.0, reach], [0.0, -reach], [reach, 0.0], [-reach, 0.0]]
                .into_iter()
                .zip(first..)
                .map(move |(offset, entity)| body(entity, team, offset))
        };
        let swinger = body(spawn.entity, 0, [0.0, 0.0]);
        let mut bodies: Vec<_> = std::iter::once(swinger)
            .chain(around(BOT_ENTITY, 0, 1.2))
            .collect();
        assert!(
            swing(&bodies).is_empty(),
            "a swing at nothing strikes no one"
        );
        bodies.extend(around(BOT_ENTITY + 4, 1, 1.5));
        let struck = swing(&bodies);
        assert_eq!(struck.len(), 1, "{struck:x?}");
        let (target, instigator, amount, heal) = struck[0];
        assert!((BOT_ENTITY + 4..BOT_ENTITY + 8).contains(&target));
        assert_eq!((instigator, amount, heal), (spawn.entity, 30.0, false));
    }

    /// Quick melee strikes the enemy where the swinger's client showed him
    /// (`0043` st5 rewinds, `m_1C2A9945`): four enemy players stand 1.5 m
    /// round Soldier: 76 (one where he looks, whichever way the spawn turns
    /// him) and run off at 12 m/s ten frames before he swings. His commands
    /// drew the world 200 ms (12.5 frames) back, so his search, rewound by
    /// the match's history of them, finds the one he faced and strikes
    /// him, though the match has them all 3.5 m away or more by then. The
    /// same swing without the history strikes nobody.
    #[test]
    #[ignore = "requires a derived 104319 Practice Range pack without bot VM graphs"]
    fn quick_melee_strikes_the_enemy_where_the_swinger_saw_him() {
        use crate::{
            combat::{Combat, PlayerTarget, PosedShot},
            replication::BodyStep,
        };
        use orpheus_wire::commands::Command;

        const MELEE: u32 = 0x800;
        /// An eighth of a turn down.
        const PITCH: i32 = 8192;
        /// The client's view delay, in ms.
        const VIEW_DELAY: u16 = 200;
        let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
        let scene = assets.prepare(GameMap::practice_range()).unwrap();
        let hero = scene.heroes().get(SOLDIER).unwrap();
        let mover = scene
            .player_spawn_mover(hero.body, 0, 0, SpawnChoice::First)
            .unwrap();
        let spawn = OwnerBodySpawn {
            slot: 0,
            controller: 0xa000_0100,
            entity: 0xa000_0101,
            body_filter: team_bit(0).unwrap(),
            frame: FIRST_TICK,
            pose: mover.pose(),
            quanta: RETAIL_QUANTA,
            mode_random: scene.mode_random(),
            ai: None,
        };
        let swing_frame = spawn.frame + 31;
        let enemies: Vec<u32> = (2..6).map(|slot| 0xa000_0001 | slot << 8).collect();
        let [x, y, z] = spawn.pose.position;
        let player = |body, team, [dx, dz]: [f64; 2]| PlayerTarget {
            body,
            team,
            pose: scene.player_target(hero, [x + dx, y, z + dz], false),
            dead: false,
            hurt: false,
            tags: None,
            ai: None,
            dealt: Vec::new(),
        };
        // Yaw 0 looks along +z.
        let players = |frame: u32| {
            let off = frame.saturating_sub(swing_frame - 10);
            let reach = 1.5 + 0.2 * f64::from(off);
            let around = [[0.0, 1.0], [0.0, -1.0], [1.0, 0.0], [-1.0, 0.0]];
            std::iter::once(player(spawn.entity, 0, [0.0; 2]))
                .chain(
                    around
                        .into_iter()
                        .zip(&enemies)
                        .map(|([dx, dz], &body)| player(body, 1, [dx * reach, dz * reach])),
                )
                .collect::<Vec<_>>()
        };
        let swing = |rewound: bool| {
            let mut combat = Combat::new(&scene);
            let mut runtime = scene.owner_body_runtime(hero, spawn).unwrap().unwrap();
            let buttons = std::iter::repeat_n(0, 30)
                .chain(std::iter::repeat_n(MELEE, 2))
                .chain(std::iter::repeat_n(0, 30));
            let mut struck = Vec::new();
            for (frame, buttons) in (spawn.frame + 1..).zip(buttons) {
                // The match records where the bodies are, then runs the
                // command (`GameService::update_combat`, `advance_input`).
                combat.record_poses(&scene, &players(frame), frame);
                runtime.set_target_candidates(&combat.target_candidates(&scene));
                let command = Command {
                    frame,
                    buttons,
                    pitch: PITCH,
                    extra: VIEW_DELAY,
                    ..Command::default()
                };
                if rewound {
                    runtime.see_targets(&combat.rewind(&scene, RETAIL_QUANTA), &command);
                }
                runtime.advance(command, &mover, BodyStep::Posed).unwrap();
                for shot in runtime.take_shots(Some(command), &mover) {
                    if let PosedShot::Health(change) = shot {
                        struck.push((change.target, change.instigator, change.amount));
                    }
                }
            }
            struck
        };
        let struck = swing(true);
        assert_eq!(struck.len(), 1, "{struck:x?}");
        let (target, instigator, amount) = struck[0];
        assert!(enemies.contains(&target), "{target:x}");
        assert_eq!((instigator, amount), (spawn.entity, 30.0));
        assert!(swing(false).is_empty(), "now they are out of reach");
    }

    #[test]
    #[ignore = "requires a derived 104319 Practice Range pack"]
    fn elevated_practice_patrols_reach_their_hints_without_crossing_walls() {
        let runtime = orpheus_testkit::runtime_dir();
        let mut assets = SceneAssets::load(runtime).unwrap();
        assets.set_bot_unknowns(Unknowns::Tolerate);
        let mut scene = assets.prepare(GameMap::practice_range()).unwrap();
        assert_eq!(scene.bots().len(), 15);
        // Every Practice Range bot body has a graph in the bot bundle.
        assert!(assets.bot_graphs().is_some());
        assert_eq!(scene.bot_scripts().pending(), 15);
        let bots = [BOT_ENTITY + 4, BOT_ENTITY + 9];
        let mut previous = bots.map(|entity| {
            scene
                .bots()
                .iter()
                .find(|bot| bot.entity == entity)
                .unwrap()
                .position
        });
        let goals = [[3, 4], [0, 1]];
        let mut visited = [0; 2];
        for tick in 1001..=2900 {
            scene.step_bots(tick);
            // The patrol graphs ask for the walks (`0B8E`).
            scene.advance_live_bots(tick);
            for (slot, entity) in bots.into_iter().enumerate() {
                let bot = scene
                    .bots()
                    .iter()
                    .find(|bot| bot.entity == entity)
                    .unwrap();
                let before = previous[slot];
                previous[slot] = bot.position;
                let delta =
                    std::array::from_fn::<_, 3, _>(|axis| bot.position[axis] - before[axis]);
                let length = delta[0].hypot(delta[1]).hypot(delta[2]);
                if length > 0.001 {
                    let origin = [before[0], before[1] + 1.05, before[2]];
                    let category = mover_category(scene.map(), bot.spawn.team);
                    let hit = scene
                        .ground
                        .cast_ray(origin, delta.map(|axis| axis / length), length, category)
                        .unwrap();
                    assert!(
                        hit.is_none_or(|hit| hit.normal[1].abs() >= 0.1),
                        "bot {entity:x} crossed a wall at {tick}: {before:?} -> {:?}, {hit:?}",
                        bot.position
                    );
                }
                if let Some(&index) = goals[slot].get(visited[slot]) {
                    let hint = bot.spawn.route[index];
                    if (bot.position[0] - hint[0]).hypot(bot.position[2] - hint[2]) < 0.06
                        && (bot.position[1] - hint[1]).abs() < 0.15
                    {
                        visited[slot] += 1;
                    }
                }
            }
        }
        assert_eq!(visited, [2; 2], "last positions {previous:?}");
    }

    /// A Practice Range wall of the visual detail (mask `0x04C0`, 10 m above
    /// the spawn): a shot along its normal meets it 1 m out, where the
    /// movement geometry, which the shots met before, has nothing within 3 m.
    #[test]
    #[ignore = "requires a derived 104319 Practice Range scene"]
    fn a_shot_meets_range_detail_the_movement_geometry_lacks() {
        let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
        let scene = assets.prepare(GameMap::practice_range()).unwrap();
        let centre = [55.025_806_427, 11.672_472_954, 41.542_596_181];
        let normal = [-0.985_830_895, 0.0, 0.167_742_206];
        let origin = [0, 1, 2].map(|axis| centre[axis] + normal[axis]);
        let back = normal.map(|axis| -axis);
        let shot = scene
            .hitscan_wall(origin, back, 3.0, None)
            .unwrap()
            .unwrap();
        assert!((shot - 1.0).abs() < 1e-6, "{shot}");
        let movement = scene
            .ground
            .cast_ray(origin, back, 3.0, MOVER_CATEGORY)
            .unwrap();
        assert!(movement.is_none(), "{movement:?}");
    }

    /// Widowmaker's Grappling Hook (`005A` st0, ray profile `0018.097`,
    /// map mask `0x0040` as the client loads it) cast straight up the
    /// Practice Range spawn passes the `0x1B9F` plane at y 11.2633 (a
    /// player clip: it stops her body, not the hook) and meets the `0x04C0`
    /// ceiling at y 12.6617. Measured in the game (2026-11-19): the
    /// client's cast end wrote (54.5654, 12.6617, 43.9388) from this origin
    /// and way; the server, casting with `0x1FDF`, stopped at the clip and
    /// pulled her client there (refs report `Grapple.md` "DayGrapple2").
    #[test]
    #[ignore = "requires a derived 104319 Practice Range scene"]
    fn the_grappling_hook_passes_the_range_spawns_player_clip() {
        const HOOK_PROFILE: u64 = 0x0690_0000_0000_0018;
        let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
        let scene = assets.prepare(GameMap::practice_range()).unwrap();
        let origin = [54.565_429_687_5, 2.5, 42.147_460_937_5];
        let way = [0.0, 0.984_815_180_301_666_3, 0.173_606_216_901_201_22];
        let hook = scene
            .map_rays()
            .cast(origin, way, 20.0, Some(HOOK_PROFILE))
            .unwrap()
            .unwrap();
        let client = [54.5654, 12.6617, 43.9388];
        assert!(
            (0..3).all(|axis| (hook.point[axis] - client[axis]).abs() < 1e-3),
            "{hook:?}"
        );
        // The clip is there, and stops a ray of the profiles' other mask.
        let clip = scene
            .rays
            .cast_ray(origin, way, 20.0, 0x1FDF)
            .unwrap()
            .unwrap();
        assert_eq!(clip.mask, 0x1B9F);
        assert!((clip.point[1] - 11.2633).abs() < 1e-3, "{clip:?}");
    }
}
