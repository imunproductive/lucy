//! Load the authorized data needed before the game worker can create a world.
//!
//! Any map of the catalog: its collision comes from the cache folder, built
//! from the install first when the cache is missing or of another version
//! (ow174/game/collision.py `_read_cache`).

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
    body_script::{OwnerBodySpawn, PlayerBodyFactory},
    bot_script::{BotEvent, BotScripts, BotShot},
    bots::{Bot, BotCatalog, BotWalk},
    combat::{BodyCombat, MeleeState, RespawnTimes, melee_states},
    controller::Controller,
    notices::switch_notices,
    pools::PoolCatalog,
    projectiles::{BioticField, FIELD_ENTITY, FieldFrames},
    replication::OwnerBodyRuntime,
    scene::{PlayerAppearance, initial_world, mode_ui},
};
use orpheus_content::{
    heroes::{EyeCatalog, Hero, HeroCatalog, SOLDIER},
    maps::{GameMap, GameModes, MODES_FILE},
    stats::{PlayerStats, StatsCatalog, initial_on_fire},
};
use orpheus_core::{bits::BitWriter, guid::parse_guid};
use orpheus_physics::{
    collision::cache::{MAGIC, VERSION, chunk_guid, read_movement_cache},
    collision::index::TriangleIndex,
    collision::scene::CollisionScene,
    contact::world::ContactWorld,
    mover::Mover,
    mover::ground::NOT_GROUND,
    mover::settings::{MoverCatalog, MoverSettings},
};
use orpheus_script::{
    codecs::CodecTable,
    graph::GraphStore,
    vm::Unknowns,
    wire::{Value, Variables},
};
use orpheus_wire::entity::{Component, EntityUpdate};

const MOVER_CATEGORY: u16 = 0x18;
/// The game mode entity's "assemble your team" seconds (ow174/game/match.py
/// `COUNTDOWN`).
const COUNTDOWN: u32 = 1303;

fn mover_category(map: &GameMap, team: u8) -> u16 {
    match (map.free_for_all, team) {
        (false, 0) => 0x08,
        (false, 1) => 0x10,
        _ => MOVER_CATEGORY,
    }
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

type OwnerBodyFactory =
    dyn Fn(&Hero, OwnerBodySpawn) -> io::Result<Option<Box<dyn OwnerBodyRuntime>>> + Send + Sync;

/// Builds a map's collision cache file from the install: (map GUID, the
/// server's map name) to the file's bytes (`orpheus_assets::collision_cache`).
pub type CollisionBuild = dyn Fn(u64, &str) -> io::Result<Vec<u8>> + Send + Sync;

pub struct SceneAssets {
    collision: PathBuf,
    build_collision: Option<Arc<CollisionBuild>>,
    /// One cache build at a time (collision.py `_building`).
    building: Mutex<()>,
    mode_props: BTreeMap<u64, BTreeSet<u64>>,
    graphs: Arc<GraphStore>,
    codecs: Option<Arc<CodecTable>>,
    /// Each mode's owner body factory, made the first time a scene of the
    /// mode is prepared.
    owner_bodies: Mutex<HashMap<u64, Option<Arc<OwnerBodyFactory>>>>,
    bot_graphs: Option<Arc<GraphStore>>,
    bot_unknowns: Unknowns,
    bots: BotCatalog,
    movers: Arc<MoverCatalog>,
    eyes: Arc<EyeCatalog>,
    respawns: RespawnTimes,
    melee: Arc<[MeleeState]>,
    /// The Biotic Field, when its graph reads; without it no field opens.
    field: Option<BioticField>,
    stats: Arc<StatsCatalog>,
    heroes: Arc<HeroCatalog>,
    pools: Arc<PoolCatalog>,
    /// The game modes' teams (`game_modes_174.json`).
    modes: GameModes,
}

pub struct PreparedScene {
    map: GameMap,
    ground: CollisionScene,
    contacts: Option<Arc<ContactWorld>>,
    controller: Controller,
    mode_ui: BitWriter,
    graphs: Arc<GraphStore>,
    initial_board: Component,
    bots: Vec<Bot>,
    bot_scripts: BotScripts,
    movers: Arc<MoverCatalog>,
    eyes: Arc<EyeCatalog>,
    /// The mode's respawn time for heroes.
    respawn: Duration,
    heroes: Arc<HeroCatalog>,
    pools: Arc<PoolCatalog>,
    stats: Arc<StatsCatalog>,
    pub owner_body_factory: Option<Arc<OwnerBodyFactory>>,
    /// The Biotic Field's frames (projectiles.py `field_frame`); without
    /// them no field is shown.
    field_frames: Option<Arc<FieldFrames>>,
    /// The mode posts hero switches in the chat (notices.py).
    switch_notices: bool,
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
        let field = match field_graph(&data.join("game_spawns_174.json"))
            .and_then(|graph| BioticField::read(&graphs, graph))
        {
            Ok(field) => Some(field),
            Err(error) => {
                eprintln!("the Biotic Field is unavailable ({error})");
                None
            }
        };
        Ok(Self {
            collision: runtime.join("collision"),
            build_collision: None,
            building: Mutex::new(()),
            mode_props: read_mode_props(&data.join("collision_props_174.json"))?,
            melee: melee_states(&graphs)?.into(),
            field,
            graphs,
            codecs,
            owner_bodies: Mutex::default(),
            bot_graphs,
            bot_unknowns: Unknowns::for_build(),
            bots: BotCatalog::load(data.join("ai_spawners_174.json"))?,
            movers: Arc::new(MoverCatalog::load(
                data.join("game_movers_174.json"),
                fallback,
            )?),
            eyes: Arc::new(EyeCatalog::load(data.join("game_eyes_174.json"), fallback)?),
            respawns: RespawnTimes::load(data.join("game_respawns_174.json"))?,
            stats: Arc::new(StatsCatalog::load(data.join("game_stats_174.json"))?),
            heroes: Arc::new(heroes),
            pools,
            modes: GameModes::load(data.join(MODES_FILE))?,
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

    /// The Biotic Field read from its graph, if it reads.
    pub fn biotic_field(&self) -> Option<&BioticField> {
        self.field.as_ref()
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

    /// Load the map before handing it to the worker. The render layouts and
    /// graph requirements are checked without blocking a game tick. A map's
    /// collision is built from the install when its cache is missing or
    /// outdated, which can take a while: callers prepare off the game thread
    /// (match.py `_refresh_collision`).
    pub fn prepare(&self, map: GameMap) -> io::Result<PreparedScene> {
        let path = self
            .collision
            .join(format!("{:04X}.col", map.map_guid as u32));
        self.ensure_collision(&map, &path)?;
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
                    ground_height(ground.triangles(), category),
                );
            }
        }
        let bot_scripts = BotScripts::new(
            self.bot_graphs.clone(),
            self.codecs
                .clone()
                .unwrap_or_else(|| Arc::new(CodecTable::client())),
            self.bot_unknowns,
            &map,
            &bots,
        );
        let controller = Controller::load(&self.graphs, map.mode_guid)?;
        let mode_ui = mode_ui(&self.graphs, &Variables::new())?;
        let initial_board =
            self.stats
                .component(&PlayerStats::default(), map.mode_guid, None, None);
        let field_frames = match (&self.codecs, &self.field) {
            (Some(codecs), Some(field)) => FieldFrames::new(&self.graphs, codecs, field)
                .map(Arc::new)
                .map_err(|error| eprintln!("the Biotic Field has no frames ({error})"))
                .ok(),
            _ => None,
        };
        let map_mode = map.mode_guid;
        let mut scene = PreparedScene {
            respawn: self.respawns.for_mode(map.mode_guid),
            map,
            ground,
            contacts,
            controller,
            mode_ui,
            graphs: Arc::clone(&self.graphs),
            initial_board,
            bots,
            bot_scripts,
            movers: Arc::clone(&self.movers),
            eyes: Arc::clone(&self.eyes),
            heroes: Arc::clone(&self.heroes),
            pools: Arc::clone(&self.pools),
            stats: Arc::clone(&self.stats),
            owner_body_factory: None,
            field_frames,
            switch_notices: switch_notices(self.modes.get(map_mode)),
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

    /// Build the map's cache from the install when it is missing or of
    /// another version, one build at a time (collision.py `_read_cache`).
    /// Without an install the cache must be there already.
    fn ensure_collision(&self, map: &GameMap, path: &Path) -> io::Result<()> {
        let Some(build) = &self.build_collision else {
            return Ok(());
        };
        if cache_version(path) == Some(VERSION) {
            return Ok(());
        }
        let _building = self
            .building
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cache_version(path) == Some(VERSION) {
            return Ok(());
        }
        let bytes = build(map.map_guid, &map.name)?;
        fs::create_dir_all(&self.collision)?;
        let temporary = path.with_extension("col.tmp");
        fs::write(&temporary, bytes)?;
        fs::rename(&temporary, path)
    }

    /// Every hero's owner body script, as Python's `BODY_SCRIPT_HEROES = None`
    /// (`ow174/game/match.py`) runs them all. Each hero's graphs are prepared
    /// the first time someone picks it, with that first spawn as the probe;
    /// a hero whose script can't start gets a body without one.
    fn prepare_owner_body(&self, scene: &PreparedScene) -> Option<Arc<OwnerBodyFactory>> {
        let graphs = Arc::clone(&self.graphs);
        let pools = Arc::clone(&self.pools);
        let eyes = Arc::clone(&self.eyes);
        let (melee, field) = (Arc::clone(&self.melee), self.field);
        let mode = scene.map.mode_guid;
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
        Some(Arc::new(move |hero, spawn| {
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
                            melee: Arc::clone(&melee),
                            field,
                        },
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
            let runtime: Box<dyn OwnerBodyRuntime> = Box::new(factory.start(spawn)?);
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
    /// side by side when the team has fewer points (match.py Player.spawn).
    pub(crate) fn player_spawn_mover(
        &self,
        body: u64,
        team: u8,
        number: usize,
    ) -> io::Result<Mover> {
        let (position, yaw) = self
            .map
            .spawn_point(team, number)
            .ok_or_else(|| invalid("no spawn for the selected hero"))?;
        let yaw = (yaw * 65536.0 / 360.0).round_ties_even() as i32;
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

    pub(crate) fn owner_body_runtime(
        &self,
        hero: &Hero,
        spawn: OwnerBodySpawn,
    ) -> io::Result<Option<Box<dyn OwnerBodyRuntime>>> {
        self.owner_body_factory
            .as_ref()
            .map_or(Ok(None), |factory| factory(hero, spawn))
    }

    pub fn heroes(&self) -> &HeroCatalog {
        &self.heroes
    }

    /// The mode's respawn time for heroes (combat.py `respawn_seconds`).
    pub fn respawn_time(&self) -> Duration {
        self.respawn
    }

    /// A body's mover settings.
    pub(crate) fn mover(&self, body: u64) -> &MoverSettings {
        self.movers.for_body(Some(body))
    }

    pub fn pools(&self) -> &PoolCatalog {
        &self.pools
    }

    pub fn controller(&self) -> Controller {
        self.controller
    }

    pub fn mode_ui(&self) -> &BitWriter {
        &self.mode_ui
    }

    /// A full frame of the game mode entity with the seconds left of
    /// "assemble your team" in v1303 (match.py `_send_countdown`,
    /// combat.py `mode_frame`).
    pub fn countdown_frame(&self, seconds: f64) -> io::Result<BitWriter> {
        mode_ui(
            &self.graphs,
            &Variables::from([(COUNTDOWN, Value::Float(seconds))]),
        )
    }

    pub fn bots(&self) -> &[Bot] {
        &self.bots
    }

    pub fn bots_mut(&mut self) -> &mut [Bot] {
        &mut self.bots
    }

    pub(crate) fn bot_target(&self, bot: &Bot) -> crate::hitscan::TargetPose {
        let settings = self.movers.for_body(Some(bot.spawn.body));
        crate::hitscan::TargetPose {
            feet: bot.position,
            height: settings.stand_height,
            radius: settings.stand_radius,
            eye: crate::bots::BOT_EYE,
        }
    }

    /// The Tab board catalog (stats.py).
    pub fn stats(&self) -> &StatsCatalog {
        &self.stats
    }

    /// The Biotic Field's frames, before and after its fade.
    pub(crate) fn field_frames(&self) -> Option<&Arc<FieldFrames>> {
        self.field_frames.as_ref()
    }

    /// A player's body as a target (combat.py `_targets`): a capsule from
    /// the feet, crouched or standing, with the mover's standing radius.
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
        }
    }

    pub(crate) fn hitscan_wall(
        &self,
        origin: [f64; 3],
        direction: [f64; 3],
        reach: f64,
    ) -> Result<Option<f64>, orpheus_physics::collision::index::IndexError> {
        self.ground
            .cast_ray(origin, direction, reach, MOVER_CATEGORY)
            .map(|hit| hit.map(|hit| hit.distance))
    }

    /// One more bot gets its script (match.py `update`: one per update,
    /// within the loading window), then the bots walk.
    pub fn step_bots(&mut self, tick: u32) {
        self.bot_scripts.prepare_next(&self.bots, tick);
        let floor = self.ground.triangles();
        for bot in &mut self.bots {
            bot.step(
                tick,
                ground_height(floor, mover_category(&self.map, bot.spawn.team)),
            );
        }
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

    /// A bot was killed: its script's death message (combat.py `_bot_died`).
    pub fn bot_died(
        &mut self,
        entity: u32,
        killer: u32,
        critical: bool,
        loadout: Option<u64>,
        tick: u32,
    ) {
        self.bot_scripts
            .died(entity, killer, critical, loadout, tick);
    }

    /// A bot's pools after a hit it survived (combat.py `_sync_body_health`).
    pub fn bot_health(&mut self, entity: u32, health: f64) {
        self.bot_scripts.set_health(entity, health);
    }

    /// The dead bots' death scripts (combat.py `_advance_bot_death`).
    pub fn advance_bot_deaths(&mut self, tick: u32) {
        self.bot_scripts.advance_deaths(&self.bots, tick);
    }

    /// The respawned bots' new scripts (combat.py `_respawn_bot`).
    pub fn prepare_respawned_bots(&mut self, tick: u32) {
        self.bot_scripts.prepare_respawned(&self.bots, tick);
    }

    /// What the bot scripts produced for the clients since the last call.
    pub fn take_bot_events(&mut self) -> Vec<BotEvent> {
        self.bot_scripts.take_events()
    }

    pub fn initial_world(&self, viewer: &PlayerAppearance<'_>) -> io::Result<Vec<EntityUpdate>> {
        initial_world(&self.map, viewer, self.initial_board.clone(), &self.bots)
    }
}

#[derive(Deserialize)]
struct PropCatalog {
    modes: BTreeMap<String, Vec<String>>,
}

/// The Biotic Field's graph: the first initial graph of [`FIELD_ENTITY`] in
/// `game_spawns_174.json` (projectiles.py `FIELD_GRAPH`, 0258).
fn field_graph(path: &Path) -> io::Result<u32> {
    #[derive(Deserialize)]
    struct Spawn {
        graphs: Vec<u32>,
    }
    let spawns: HashMap<String, Spawn> =
        serde_json::from_str(&fs::read_to_string(path)?).map_err(invalid)?;
    spawns
        .get(&format!("0x{FIELD_ENTITY:016X}"))
        .and_then(|spawn| spawn.graphs.first().copied())
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

#[cfg(test)]
mod ported_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bots::BOT_ENTITY, instance::RETAIL_QUANTA, scene::team_bit};
    use orpheus_content::heroes::SOLDIER;
    use orpheus_core::tick::FIRST_TICK;

    #[test]
    #[ignore = "requires a derived 104319 Practice Range pack without bot VM graphs"]
    fn stock_heroes_attach_their_authored_owner_scripts() {
        let runtime = orpheus_testkit::runtime_dir();
        let assets = SceneAssets::load(runtime).unwrap();
        let scene = assets.prepare(GameMap::practice_range()).unwrap();
        assert!(scene.movement_ready());
        let hero = scene.heroes().get(SOLDIER).unwrap();
        let spawn = OwnerBodySpawn {
            slot: 0,
            controller: 0xa000_0100,
            entity: 0xa000_0101,
            body_filter: team_bit(0).unwrap(),
            frame: FIRST_TICK,
            pose: scene.player_spawn_mover(hero.body, 0, 0).unwrap().pose(),
            quanta: RETAIL_QUANTA,
        };
        let soldier = scene.owner_body_runtime(hero, spawn).unwrap();
        assert!(
            soldier.is_some(),
            "the stock Soldier: 76 owner script did not start"
        );
        // Every hero has a script, not only Soldier: 76 (Pharah's starts with
        // no unknown content).
        let pharah = scene.heroes().find_hero("Pharah").unwrap();
        let spawn = OwnerBodySpawn {
            pose: scene.player_spawn_mover(pharah.body, 0, 0).unwrap().pose(),
            ..spawn
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
        let mover = scene.player_spawn_mover(hero.body, 0, 0).unwrap();
        let spawn = OwnerBodySpawn {
            slot: 0,
            controller: 0xa000_0100,
            entity: 0xa000_0101,
            body_filter: team_bit(0).unwrap(),
            frame: FIRST_TICK,
            pose: mover.pose(),
            quanta: RETAIL_QUANTA,
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
        // Every shot of the session, Helix and the quick melee included; a
        // change here means the weapons' timing changed.
        assert_eq!(shots, 32);
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
                (0x0043, Some(44), "STU_6E5C5F66", "no-op"),
                (0x0043, Some(8), "STU_1C2CC6D5", "no-op"),
                (
                    0x0043,
                    Some(11),
                    "STUStatescriptStateTrackTargets",
                    "finds no targets"
                ),
                (0x0043, Some(9), "STU_23A00A53", "no effect"),
                (
                    0x0043,
                    Some(10),
                    "STUStatescriptStateModifyHealth",
                    "no effect"
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
        // The frames encode because the one map value is left out of them.
        let maps: Vec<_> = snapshot
            .instances
            .values()
            .flat_map(|instance| {
                instance
                    .variables
                    .iter()
                    .filter(|(_, value)| matches!(value, orpheus_script::wire::Value::Map(_)))
                    .map(|(&variable, _)| (instance.graph, variable))
            })
            .collect();
        assert_eq!(maps, [(0x05ad, 29859)]);
    }

    #[test]
    #[ignore = "requires a derived 104319 Practice Range pack"]
    fn elevated_practice_patrols_reach_their_hints_without_crossing_walls() {
        let runtime = orpheus_testkit::runtime_dir();
        let assets = SceneAssets::load(runtime).unwrap();
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
}
