//! Tracer's Blink and Recall through the player's tick: her body script on
//! the derived Practice Range scene, with its collision. Her graphs still
//! reach config classes the VM doesn't compute (`STU_60146072`,
//! `STU_038C32E8`, `STU_082FA44E`) and `StateBlinkEffect`, none of which
//! decides the moves tested here, so the body tolerates them as release
//! builds do.

use super::*;
use orpheus_script::{vm::Unknowns, wire::Value as ScriptValue};
use orpheus_wire::commands::Command;

const TRACER: u64 = 0x02e0_0000_0000_0003;
/// Shift and E on the command (logical buttons 35 and 36).
const BLINK: u32 = 0x08;
const RECALL: u32 = 0x10;
const DT: f64 = FRAME_MS as f64 / 1000.0;

struct Tracer {
    scene: PreparedScene,
    world: WorldDelivery,
    frame: u32,
    card: PlayerCard,
}

impl Tracer {
    /// Tracer picked at the Practice Range spawn, standing for a second.
    fn spawn() -> Self {
        let mut assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
        assets.set_player_unknowns(Unknowns::Tolerate);
        let scene = assets.prepare(GameMap::practice_range()).unwrap();
        let mut tracer = Self {
            scene,
            world: WorldDelivery::new(OWNER),
            frame: FIRST_TICK,
            card: PlayerCard::default(),
        };
        tracer.world.started = Some(FIRST_TICK);
        let hero = tracer.scene.heroes().get(TRACER).unwrap();
        let viewer = PlayerAppearance {
            entity: OWNER,
            slot: 0,
            team: 0,
            spawn: 0,
            room: orpheus_content::maps::SpawnChoice::First,
            card: &tracer.card,
        };
        tracer
            .world
            .switch(
                &tracer.scene,
                &viewer,
                FIRST_TICK,
                RETAIL_QUANTA,
                OWNER | 1,
                (hero, SkinTheme::default()),
                true,
            )
            .unwrap();
        assert!(tracer.world.body_script.is_some(), "Tracer's body script");
        tracer.idle(60, 0);
        tracer
    }

    fn step(&mut self, command: Command) {
        self.frame += 1;
        let command = Command {
            frame: self.frame,
            ..command
        };
        let viewer = PlayerAppearance {
            entity: OWNER,
            slot: 0,
            team: 0,
            spawn: 0,
            room: orpheus_content::maps::SpawnChoice::First,
            card: &self.card,
        };
        self.world.queue_commands(self.frame, &[command]);
        while self.world.has_input_work() {
            self.world
                .advance_input(&self.scene, &viewer, RETAIL_QUANTA, 64)
                .unwrap();
        }
        assert!(self.world.body_script.is_some(), "the script kept running");
    }

    fn idle(&mut self, frames: u32, yaw: i32) {
        for _ in 0..frames {
            self.step(Command {
                yaw,
                ..Command::default()
            });
        }
    }

    /// One press and release of `button`, facing `yaw`.
    fn press(&mut self, button: u32, yaw: i32) {
        self.step(Command {
            yaw,
            buttons: button,
            ..Command::default()
        });
        self.step(Command {
            yaw,
            ..Command::default()
        });
    }

    fn position(&self) -> [f64; 3] {
        self.world.mover.as_ref().unwrap().pose().position
    }

    fn snapshot(&self) -> &Snapshot {
        self.world
            .body_script
            .as_ref()
            .unwrap()
            .runtime
            .snapshot()
            .unwrap()
    }

    /// Blink's charges: variable `000B` of graph `0024`.
    fn charges(&self) -> i64 {
        let (_, blink) = self
            .snapshot()
            .instances
            .iter()
            .find(|(_, instance)| instance.graph == 0x0024)
            .unwrap();
        match blink.variables.get(&0x000b) {
            Some(ScriptValue::Integer(value)) => *value,
            Some(ScriptValue::Float(value)) => *value as i64,
            other => panic!("charges {other:?}"),
        }
    }

    fn rewinding(&self) -> bool {
        self.snapshot()
            .instances
            .values()
            .find(|instance| instance.graph == 0x001a)
            .and_then(|instance| instance.states.get(&0))
            .is_some_and(|state| state.active)
    }
}

fn horizontal(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

/// How far a dash of 7.5 m in 0.1 s gets from the settled spawn facing
/// `yaw`, run on the mover alone.
fn mover_dash(scene: &PreparedScene, yaw: i32) -> f64 {
    let hero = scene.heroes().get(TRACER).unwrap();
    let mut mover = scene
        .player_spawn_mover(hero.body, 0, 0, orpheus_content::maps::SpawnChoice::First)
        .unwrap();
    let rays = scene.movement_index();
    let command = |frame| Command {
        frame,
        yaw,
        ..Command::default()
    };
    for frame in 1..=61 {
        mover.step_pose_to(rays, command(frame), DT, None).unwrap();
    }
    let start = mover.pose().position;
    mover.dash(7.5, 0.1);
    let mut frame = 61;
    while mover.authored_motion() {
        frame += 1;
        mover.step_pose_to(rays, command(frame), DT, None).unwrap();
    }
    for _ in 0..30 {
        frame += 1;
        mover.step_pose_to(rays, command(frame), DT, None).unwrap();
    }
    horizontal(start, mover.pose().position)
}

/// A yaw (in 1/65536 turns) where a dash from the spawn runs its full 7.5 m
/// and one where a wall stops it short.
fn open_and_walled(scene: &PreparedScene) -> (i32, i32) {
    let reached: Vec<(i32, f64)> = (0..16)
        .map(|eighth| eighth * 4096)
        .map(|yaw| (yaw, mover_dash(scene, yaw)))
        .collect();
    let open = reached
        .iter()
        .find(|(_, distance)| (distance - 7.5).abs() < 0.05)
        .unwrap_or_else(|| panic!("no open direction: {reached:?}"));
    let walled = reached
        .iter()
        .find(|(_, distance)| *distance < 6.0)
        .unwrap_or_else(|| panic!("no wall in reach: {reached:?}"));
    (open.0, walled.0)
}

/// Blink moves Tracer 7.5 m (graph `0024`'s `STU_A807207A`), takes one of
/// her three charges per press, refuses a press with none left and gives
/// one back every 3 s; Recall takes her back to where she was 3 s before.
#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn tracer_blinks_spends_charges_and_recalls() {
    let mut tracer = Tracer::spawn();
    let (open, _) = open_and_walled(&tracer.scene);
    assert_eq!(tracer.charges(), 3);

    let start = tracer.position();
    let first = tracer.frame + 1;
    tracer.press(BLINK, open);
    tracer.idle(30, open);
    let after = tracer.position();
    assert!(
        (horizontal(start, after) - 7.5).abs() < 0.05,
        "{start:?} -> {after:?}"
    );
    assert_eq!(tracer.charges(), 2);

    for left in [1, 0] {
        tracer.press(BLINK, open);
        tracer.idle(10, open);
        assert_eq!(tracer.charges(), left);
    }
    // No charge: the press doesn't move her.
    let spent = tracer.position();
    tracer.press(BLINK, open);
    tracer.idle(10, open);
    assert_eq!(tracer.charges(), 0);
    assert!(horizontal(tracer.position(), spent) < 0.01);

    // One charge back every 3 s from the first spend (graph 0024's ChaseVar
    // toward variable 01D8, 3 s): 187.5 frames of 16 ms.
    let until = |tracer: &mut Tracer, frame: u32| {
        let frames = frame - tracer.frame;
        tracer.idle(frames, open);
    };
    until(&mut tracer, first + 182);
    assert_eq!(tracer.charges(), 0);
    until(&mut tracer, first + 194);
    assert_eq!(tracer.charges(), 1);
    until(&mut tracer, first + 370);
    assert_eq!(tracer.charges(), 1);
    until(&mut tracer, first + 382);
    assert_eq!(tracer.charges(), 2);

    // Walk back for a second, then Recall.
    let mut path = BTreeMap::new();
    for _ in 0..60 {
        tracer.step(Command {
            yaw: open,
            forward: -127,
            ..Command::default()
        });
        path.insert(tracer.frame, tracer.position());
    }
    for _ in 0..140 {
        tracer.idle(1, open);
        path.insert(tracer.frame, tracer.position());
    }
    let walked = tracer.position();
    let mut began = None;
    tracer.step(Command {
        yaw: open,
        buttons: RECALL,
        ..Command::default()
    });
    let press = tracer.frame;
    for _ in 0..120 {
        if began.is_none() && tracer.rewinding() {
            began = Some(tracer.frame);
        }
        tracer.idle(1, open);
    }
    // The wind-up ends at once: the rewind starts with the press.
    assert_eq!(began, Some(press));
    assert!(!tracer.rewinding());
    // The rewind starts from the pose of the press and goes 3 s (188 poses
    // of 16 ms) back, which is 12 poses into the walk.
    let then = path[&(press - 188)];
    let back = tracer.position();
    assert!(horizontal(back, walked) > 1.0);
    assert!(horizontal(back, then) < 0.001, "{back:?} vs {then:?}");
}

/// Blink stops at the wall the mover meets, short of 7.5 m.
#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn tracer_blink_stops_at_a_wall() {
    let mut tracer = Tracer::spawn();
    let (_, walled) = open_and_walled(&tracer.scene);
    let expected = mover_dash(&tracer.scene, walled);
    let start = tracer.position();
    tracer.press(BLINK, walled);
    tracer.idle(30, walled);
    let moved = horizontal(start, tracer.position());
    assert!(moved < 6.0, "{moved}");
    assert!((moved - expected).abs() < 0.05, "{moved} vs {expected}");
    assert_eq!(tracer.charges(), 2);
}
