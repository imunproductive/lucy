//! A Workshop script over a player's ticks ([`crate::workshop`]): C32S2,
//! the Doomfist parkour of Hanamura (the DPK archive), on Hanamura Assault
//! with one Doomfist, its actions as their graphs on the mode entity
//! ([`ModeRunner::workshop`]).

use super::*;
use crate::{
    match_flow::ASSAULT,
    mode_script::ModeRunner,
    scene_assets::install_collision_build,
    workshop::{WorkshopPlayer, WorkshopRun},
};
use orpheus_content::maps::{GameModes, MODES_FILE, MapCatalog, SpawnChoice};
use orpheus_script::{
    mode::ModePlayer,
    wire::{Value as ScriptValue, Variables},
};
use orpheus_workshop::{Catalog, Logic, parse};

const DOOMFIST: u64 = 0x02e0_0000_0000_012f;
const CREATE_HUD_TEXT: u32 = 0x1d8a;
const CREATE_EFFECT: u32 = 0x1ce8;
/// Create Effect's entity (`1CE8` rid6 `STUStatescriptActionCreateEntity`)
/// and its scope there (st13).
const EFFECT_ENTITY: u64 = 0x0400_0000_0000_304a;
const EFFECT_SCOPE: u32 = 35;
/// Create HUD Text's scope on the mode entity (`1D8A` st36).
const HUD_TEXT_SCOPE: u32 = 31;

struct Parkour {
    scene: PreparedScene,
    card: PlayerCard,
    world: WorldDelivery,
    mode: ModeRunner,
    run: WorkshopRun,
    logic: Logic,
    frame: u32,
}

impl Parkour {
    /// C32S2 on Hanamura Assault, its player's Doomfist at his spawn.
    fn start() -> Self {
        let runtime = orpheus_testkit::runtime_dir();
        let data = runtime.join("data");
        let catalog = Arc::new(Catalog::load(&data).unwrap());
        let path =
            orpheus_testkit::dpk_archive().join("workshop.codes/C32S2_OG_doom_parkur_hanamura.txt");
        let logic = parse(&std::fs::read_to_string(path).unwrap(), &catalog)
            .unwrap()
            .logic;
        let maps = MapCatalog::load(&data)
            .unwrap()
            .with_modes(GameModes::load(data.join(MODES_FILE)).unwrap());
        let hanamura = maps
            .map_catalog()
            .into_iter()
            .find(|listing| listing.name == "Hanamura")
            .unwrap();
        let map = maps
            .game_map(hanamura.guid, ASSAULT, "Assault", false, &[6, 6])
            .unwrap();
        let mut assets = SceneAssets::load(&runtime)
            .unwrap()
            .with_collision_build(install_collision_build(orpheus_testkit::game_dir()));
        assets.set_player_unknowns(orpheus_script::vm::Unknowns::Tolerate);
        let scene = assets.prepare(map).unwrap();
        let card = PlayerCard::default();
        let mut world = WorldDelivery::new(OWNER).unbuffered();
        let viewer = viewer(&card);
        world.start(&scene, &viewer, FIRST_TICK).unwrap();
        let hero = scene.heroes().get(DOOMFIST).unwrap();
        world
            .switch(
                &scene,
                &viewer,
                FIRST_TICK,
                RETAIL_QUANTA,
                OWNER | 1,
                (hero, SkinTheme::default()),
                true,
            )
            .unwrap();
        let players = vec![ModePlayer {
            controller: OWNER,
            body: world.current_body(),
            hero: world.current_hero(),
            team: 0,
        }];
        let mode = ModeRunner::workshop(&scene, FIRST_TICK, players).expect("a Workshop host");
        Self {
            run: WorkshopRun::new(&logic, catalog, 7),
            scene,
            card,
            world,
            mode,
            logic,
            frame: FIRST_TICK,
        }
    }

    /// One server tick: the player's idle command, the mode entity, the
    /// rules.
    fn tick(&mut self) {
        self.frame += 1;
        let viewer = viewer(&self.card);
        self.world.queue_commands(
            self.frame,
            &[Command {
                frame: self.frame,
                ..Command::default()
            }],
        );
        while self.world.has_input_work() {
            self.world
                .advance_input(&self.scene, &viewer, RETAIL_QUANTA, 64, None)
                .unwrap();
        }
        let players = vec![ModePlayer {
            controller: OWNER,
            body: self.world.current_body(),
            hero: self.world.current_hero(),
            team: 0,
        }];
        self.mode.advance(self.frame, &Variables::new(), players);
        let mut players = [WorkshopPlayer {
            controller: OWNER,
            slot: 0,
            team: 0,
            world: &mut self.world,
        }];
        self.run
            .step(self.frame, &self.scene, &mut self.mode, &mut players);
        assert_eq!(self.run.engine().faults(), [] as [String; 0]);
    }

    fn run(&mut self, seconds: f64) {
        for _ in 0..(seconds / 0.016).round() as u32 {
            self.tick();
        }
    }

    fn player_variable(&self, name: &str) -> ScriptValue {
        let index = self
            .logic
            .player_names
            .iter()
            .position(|item| item == name)
            .unwrap();
        self.world.workshop_variable(index as u8).unwrap()
    }

    fn global(&mut self, name: &str) -> ScriptValue {
        let index = self
            .logic
            .global_names
            .iter()
            .position(|item| item == name)
            .unwrap();
        self.mode
            .script_mut()
            .unwrap()
            .workshop_global(index as i64)
    }

    /// The first checkpoint's centre and radius (`QCPdata[0][3]`, `[4]`).
    fn first_checkpoint(&mut self) -> ([f64; 3], f64) {
        let ScriptValue::Array(checkpoints) = self.global("QCPdata") else {
            panic!("no checkpoint data");
        };
        let ScriptValue::Array(first) = &checkpoints[0] else {
            panic!("{:?}", checkpoints[0]);
        };
        let (ScriptValue::Vector(centre), radius) = (&first[3], &first[4]) else {
            panic!("{first:?}");
        };
        (*centre, number(radius))
    }

    fn feet(&self) -> [f64; 3] {
        self.world.mover.as_ref().unwrap().pose().position
    }

    fn on_ground(&self) -> bool {
        self.world.mover.as_ref().unwrap().on_ground()
    }

    /// Puts the body's feet at `position`.
    fn place(&mut self, position: [f64; 3]) {
        let body = self.scene.heroes().get(DOOMFIST).unwrap().body;
        self.world.mover = Some(self.scene.player_mover_at(body, 0, position, 0).unwrap());
    }

    /// The mode entity's instances of `graph` and their scope ids.
    fn instances(&self, graph: u32) -> Vec<Option<u32>> {
        let ModeRunner::Graph { script, .. } = &self.mode else {
            panic!("no graph drives the mode entity");
        };
        script
            .snapshot()
            .unwrap()
            .instances
            .values()
            .filter(|instance| instance.graph == graph)
            .map(|instance| instance.scope.as_ref().map(|scope| scope.id))
            .collect()
    }
}

fn viewer(card: &PlayerCard) -> PlayerAppearance<'_> {
    PlayerAppearance {
        entity: OWNER,
        slot: 0,
        team: 0,
        spawn: 0,
        room: SpawnChoice::First,
        card,
    }
}

/// A Workshop number: False and True are 0 and 1 (Reset sets RunState to
/// False).
fn number(value: &ScriptValue) -> f64 {
    match value {
        ScriptValue::Float(value) => *value,
        ScriptValue::Integer(value) => *value as f64,
        ScriptValue::Bool(value) => f64::from(u8::from(*value)),
        ScriptValue::Null => 0.0,
        other => panic!("{other:?}"),
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3)
        .map(|axis| (a[axis] - b[axis]).powi(2))
        .sum::<f64>()
        .sqrt()
}

/// The player who joins is reset to the first checkpoint ("Player Joins"
/// calls Reset once he has spawned; Reset's CP Change teleports him to
/// `CPData[3]`, Teleport `1D45`'s scope on his controller moving the body),
/// and his HUD comes up: "Player HUD" creates six HUD texts within 4 s
/// (its waits are random, at most 2 + 2 s), each Create HUD Text `1D8A`
/// keeping its scope 31 on the mode entity for the clients' presenters;
/// "Player Joins" creates the checkpoint's ring (Create Effect `1CE8`).
/// The mode entity's frame, with all of it, can be made.
#[test]
#[ignore = "requires an Overwatch 1.74 install and the DPK archive (just test-dpk <install> <archive>)"]
fn dpk_archive_c32s2_resets_the_player_to_the_first_checkpoint_with_his_hud() {
    let mut parkour = Parkour::start();
    parkour.run(5.0);
    let (centre, radius) = parkour.first_checkpoint();
    assert!(
        distance(parkour.feet(), centre) <= radius + 0.5,
        "at {:?}, the checkpoint at {centre:?} ({radius} m)",
        parkour.feet()
    );
    let scopes = parkour
        .instances(CREATE_HUD_TEXT)
        .into_iter()
        .filter(|scope| *scope == Some(HUD_TEXT_SCOPE))
        .count();
    assert!(scopes >= 6, "{scopes} HUD texts");
    assert!(!parkour.instances(CREATE_EFFECT).is_empty());
    let gaps = parkour.mode.script_mut().unwrap().gaps();
    assert!(parkour.mode.project(None).is_some(), "{gaps:?}");
    // The ring: Create Effect made a `304A`, opened its scope 35 there
    // (`1CE8` st13, the presenters), and its frame can be sent.
    let rings: Vec<u32> = parkour
        .mode
        .created()
        .filter(|made| made.entity.definition == EFFECT_ENTITY && made.frame().is_some())
        .map(|made| made.entity.entity)
        .collect();
    let script = parkour.mode.script_mut().unwrap();
    assert!(
        rings.iter().any(|&ring| {
            script
                .created_script(ring)
                .and_then(|placed| placed.snapshot())
                .is_some_and(|snapshot| {
                    snapshot.instances.values().any(|instance| {
                        instance.graph == CREATE_EFFECT
                            && instance.scope.as_ref().map(|scope| scope.id) == Some(EFFECT_SCOPE)
                    })
                })
        }),
        "no ring among {rings:08X?}"
    );
}

/// The run's timer: leaving the first checkpoint in the air starts it
/// ("Timer Start": RunState 1, Chase Player Variable At Rate `1CCC` playing
/// `1CCF` on the controller, RunTime up 1 a second); landing outside the
/// checkpoint fails it in the tick it lands ("CP Clear / Fail", on Is On
/// Ground → CP Fail → Reset): back at the checkpoint, the chase stopped
/// (Stop Chasing `1CCE`'s `0DCF` ends the controller's `1CCF`), RunTime 0
/// and no longer moving.
#[test]
#[ignore = "requires an Overwatch 1.74 install and the DPK archive (just test-dpk <install> <archive>)"]
fn dpk_archive_c32s2_times_the_run_and_resets_it_on_a_fall() {
    let mut parkour = Parkour::start();
    parkour.run(5.0);
    let (centre, radius) = parkour.first_checkpoint();
    parkour.place([centre[0] + radius + 2.0, centre[1] + 12.0, centre[2]]);
    let mut started = None;
    let mut airborne = None;
    let mut ticks = 0;
    // A mover just put somewhere reads as grounded until it first moves:
    // the fall is seen from the first tick on.
    loop {
        parkour.tick();
        ticks += 1;
        assert!(ticks < 300, "never lands");
        if parkour.on_ground() {
            break;
        }
        if started.is_none() && number(&parkour.player_variable("RunState")) == 1.0 {
            started = Some(parkour.frame);
        }
        airborne = Some((parkour.frame, number(&parkour.player_variable("RunTime"))));
    }
    let (Some(started), Some((last, time))) = (started, airborne) else {
        let gaps = parkour.mode.script_mut().unwrap().gaps();
        panic!(
            "the timer didn't start in the air ({ticks} ticks, RunState {:?}, gaps {gaps:?})",
            parkour.player_variable("RunState")
        );
    };
    // The last tick in the air, before the landing's reset.
    let expected = f64::from(last - started) * 0.016;
    assert!(
        (time - expected).abs() <= 0.05,
        "RunTime {time} after {expected} s"
    );
    parkour.run(0.5);
    assert!(distance(parkour.feet(), centre) <= radius + 0.5);
    assert_eq!(number(&parkour.player_variable("RunTime")), 0.0);
    assert_eq!(number(&parkour.player_variable("RunState")), 0.0);
}

/// Level select: from the hub (checkpoint 0, whose ring list `QPos[0]`
/// holds the levels' first checkpoints), landing in level 1's ring
/// (checkpoint 1, 1.872 m, at (18.363, 4.117, 9.094)) clears it ("CP Clear
/// / Fail" → CP Clear: the checkpoint is 1, "First CP of a Level" sets the
/// level to 1, CP Change puts him at its centre). The hub's list is whole
/// only if MLVLicons' Abort returns to "Generate Global Data" (it ends the
/// subroutine, not the rule).
///
/// The player's first Reset ran on his spawn, before "Generate Global Data"
/// finished (here he spawns with the match; in a game he picks his hero
/// later), so his list was the hub's at that time: a fall resets him and
/// takes the hub's list again.
#[test]
#[ignore = "requires an Overwatch 1.74 install and the DPK archive (just test-dpk <install> <archive>)"]
fn dpk_archive_c32s2_landing_in_level_ones_ring_selects_it() {
    const LEVEL_ONE: [f64; 3] = [18.363, 4.117, 9.094];
    let mut parkour = Parkour::start();
    parkour.run(5.0);
    let ScriptValue::Array(hub) = parkour.global("QPos") else {
        panic!("no checkpoint positions");
    };
    let ScriptValue::Array(levels) = &hub[0] else {
        panic!("{:?}", hub[0]);
    };
    // 11 levels: level 1 to 10 and the secret one.
    assert_eq!(levels.len(), 11);
    assert_eq!(levels[0], ScriptValue::Vector(LEVEL_ONE));
    let (centre, _) = parkour.first_checkpoint();
    // The ground below the hub's balcony, 2.1 m from level 1's centre.
    parkour.place([20.5, 5.0, 9.6]);
    parkour.run(1.5);
    assert!(distance(parkour.feet(), centre) < 1.0, "the fall reset him");
    parkour.place([18.5, 5.5, 9.1]);
    parkour.run(1.0);
    assert_eq!(number(&parkour.player_variable("Checkpoint")), 1.0);
    assert_eq!(number(&parkour.player_variable("Level")), 1.0);
    assert!(
        distance(parkour.feet(), LEVEL_ONE) < 0.5,
        "{:?}",
        parkour.feet()
    );
}
