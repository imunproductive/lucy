//! Heroes' moves through the player's tick: the body script on the derived
//! Practice Range scene, with its collision. Tracer's Blink and Recall;
//! Pharah's Jump Jet; Echo's ground ray. Tracer's graphs still reach a config class the VM
//! doesn't compute (`STU_038C32E8`) and `StateBlinkEffect`, neither of
//! which decides the moves tested here, so the body tolerates unknowns as
//! release builds do.

use super::*;
use orpheus_script::{
    vm::{State, StateKind, Unknowns},
    wire::Value as ScriptValue,
};
use orpheus_wire::{commands::Command, entity::MOVE_TO};

const TRACER: u64 = 0x02e0_0000_0000_0003;
const PHARAH: u64 = 0x02e0_0000_0000_0008;
const ECHO: u64 = 0x02e0_0000_0000_0206;
/// Shift and E on the command (logical buttons 35 and 36).
const BLINK: u32 = 0x08;
const RECALL: u32 = 0x10;
const DT: f64 = FRAME_MS as f64 / 1000.0;
/// The movement state's flags "fell" (taken off without a jump) and "the
/// last move touched something".
const FELL: u32 = 0x8;
const TOUCHING: u32 = 0x2_0000;

struct Player {
    scene: PreparedScene,
    world: WorldDelivery,
    frame: u32,
    card: PlayerCard,
}

impl Player {
    /// `hero` picked at the Practice Range spawn, standing for a second.
    fn spawn(hero: u64) -> Self {
        Self::spawn_in(hero, WorldDelivery::new(OWNER).unbuffered())
    }

    /// `hero` played by an AI at the Practice Range spawn
    /// ([`WorldDelivery::headless`]): his AI entity possesses the body.
    fn spawn_ai(hero: u64) -> Self {
        let difficulty = crate::ai::Difficulty::Hard.identifier();
        Self::spawn_in(
            hero,
            WorldDelivery::headless(OWNER, 0x8000_7001, difficulty),
        )
    }

    fn spawn_in(hero: u64, world: WorldDelivery) -> Self {
        let mut assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
        assets.set_player_unknowns(Unknowns::Tolerate);
        let scene = assets.prepare(GameMap::practice_range()).unwrap();
        let mut player = Self {
            scene,
            world,
            frame: FIRST_TICK,
            card: PlayerCard::default(),
        };
        player.world.started = Some(FIRST_TICK);
        let hero = player.scene.heroes().get(hero).unwrap();
        let viewer = PlayerAppearance {
            entity: OWNER,
            slot: 0,
            team: 0,
            spawn: 0,
            room: orpheus_content::maps::SpawnChoice::First,
            card: &player.card,
        };
        player
            .world
            .switch(
                &player.scene,
                &viewer,
                FIRST_TICK,
                RETAIL_QUANTA,
                OWNER | 1,
                (hero, SkinTheme::default()),
                true,
            )
            .unwrap();
        assert!(player.world.body_script.is_some(), "the hero's body script");
        player.idle(60, 0);
        player
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
                .advance_input(&self.scene, &viewer, RETAIL_QUANTA, 64, None)
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

    /// Recall's rewind (`001A` st0) while it runs.
    fn rewind(&self) -> Option<&State> {
        self.snapshot()
            .instances
            .values()
            .find(|instance| instance.graph == 0x001a)
            .and_then(|instance| instance.states.get(&0))
            .filter(|state| state.active)
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
    let mut tracer = Player::spawn(TRACER);
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
    let until = |tracer: &mut Player, frame: u32| {
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
        if began.is_none() {
            began = tracer
                .rewind()
                .map(|state| (tracer.frame, state.start_ms, state.kind));
        }
        tracer.idle(1, open);
    }
    // The wind-up ends at once: the rewind starts with the press.
    let (frame, start_ms, kind) = began.unwrap();
    assert_eq!(frame, press);
    assert!(tracer.rewind().is_none());
    // Its frames carry its end, 0.9 s after its begin, and its 3 s of
    // history in frames, rounded up (`begin_rewind`).
    assert_eq!(
        kind,
        StateKind::Rewind {
            end_ms: start_ms + 900,
            frames: 188
        }
    );
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
    let mut tracer = Player::spawn(TRACER);
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

/// The target of Widowmaker's grapple pull (`005A` st1, a `MoveTo` with
/// `m_EA99616F`) is the free spot of a temporary character at the hook's
/// destination (`0x7FF789C4A8A0`), as her client puts it. Recorded in the
/// game (2026-11-19, owdbg probe of the kind-1 begin `0x7FF789B53CC0`,
/// block `+0x20` target, `+0x30` destination, `+0x50` surface; refs
/// report `Grapple.md` "DayGrapple2"): under the Range spawn's ceiling
/// (the `0x1B9F` player clip at y 11.2633, normal down) the capsule is
/// stopped 0.075 m below the destination, not 2 m (the goal, her height
/// below it); at a wall it ends 0.08 m past the goal. The other three are
/// from a later session (`Grapple.md` "DayGrapple3"): the `0x04C0`
/// ceiling above the spawn, a ledge face, and a wall's top edge, where the
/// capsule stands on the top only 0.06 m out.
#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn widowmakers_pull_targets_the_free_spot_her_client_finds() {
    use orpheus_physics::mover::{Behaviour, MoveKey, MoveTo, Speed};
    const WIDOWMAKER: u64 = 0x02e0_0000_0000_000a;
    let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
    let body = scene.heroes().get(WIDOWMAKER).unwrap().body;
    // Feet, destination, surface, the client's target (f32 as recorded).
    let recorded: [[[f32; 3]; 4]; 5] = [
        [
            [54.565_43, 1.0, 42.147_46],
            [54.565_43, 11.263_25, 43.692_272],
            [0.0, -1.0, 0.0],
            [54.565_43, 11.188_249, 43.692_272],
        ],
        [
            [44.0, 1.0, 22.0],
            [48.399_193, 2.726_54, 28.485_128],
            [0.167_743, -0.0, 0.985_831],
            [48.535_904, 1.726_54, 29.040_054],
        ],
        [
            [54.565_43, 1.0, 42.147_46],
            [54.565_43, 12.661_688, 43.938_793],
            [0.0, -1.0, 0.0],
            [54.565_43, 11.193_249, 43.938_793],
        ],
        [
            [54.565_43, 2.171_875, 44.095_703],
            [54.565_43, 4.130_847, 45.696_16],
            [-0.167_74, 0.0, -0.985_831],
            [54.479_885, 3.130_847, 45.193_398],
        ],
        [
            [54.479_492, 2.162_109, 45.193_36],
            [54.479_492, 6.476_256, 29.229_56],
            [0.167_743, 0.0, 0.985_831],
            [54.489_937, 6.420_997, 29.290_888],
        ],
    ];
    for [feet, destination, surface, client] in recorded {
        let feet = feet.map(f64::from);
        let mut mover = scene.player_mover_at(body, 0, feet, 0).unwrap();
        let pull = MoveTo {
            destination: destination.map(f64::from),
            surface: surface.map(f64::from),
            free_spot: true,
            speed: Speed::MetresPerSecond(20.0),
            acceleration: 100.0,
            slow_radius: 0.0,
            lateral_seconds: 0.3,
            stuck_distance: 1.0,
            stuck_seconds: 0.3,
            exit_horizontal: 10_000.0,
            exit_vertical: 10_000.0,
            exit_total: 10_000.0,
            exit_on_end: false,
            abort_past: true,
        };
        let key = MoveKey {
            instance: 1,
            raw_state: 1,
            activation: 1,
        };
        let command = Command {
            frame: 1,
            ..Command::default()
        };
        mover
            .step_pose_to(
                scene.movement_index(),
                command,
                DT,
                Some((key, &Behaviour::MoveTo(pull))),
            )
            .unwrap();
        let target = mover.move_target().unwrap();
        // Within a few f32 steps of the recorded values.
        assert!(
            (0..3).all(|axis| (target[axis] - f64::from(client[axis])).abs() < 2e-5),
            "{target:?} vs the client's {client:?}"
        );
    }
}

/// Widowmaker's pull from the Range spawn with the kind-1 block her client
/// began (destination `+0x30`, surface `+0x40`, `005A` st1's numbers), then
/// with `walk` the ledge pull `005A` case 2 hands the mover from the next
/// frame (`STU_1F5ED928` st30: the hook point, 0.2 s) until the mover
/// reports it done, then `after` frames without either: how many frames
/// the pull drove, how it ended, how many the walk took, and the movement
/// state (as a correction carries it) on every frame.
struct Pulled {
    mover: orpheus_physics::mover::Mover,
    driven: u32,
    end: orpheus_physics::mover::MoveEnd,
    velocity: [f64; 3],
    walked: u32,
    path: Vec<[f64; 3]>,
    poses: Vec<Movement>,
}

fn pull_from_the_spawn(
    destination: [f64; 3],
    surface: [f64; 3],
    walk: Option<[f64; 3]>,
    after: u32,
) -> Pulled {
    use orpheus_physics::mover::{Behaviour, MoveKey, MoveTo, PathWalk, Speed};
    const WIDOWMAKER: u64 = 0x02e0_0000_0000_000a;
    let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
    let body = scene.heroes().get(WIDOWMAKER).unwrap().body;
    let rays = scene.movement_index();
    let mut mover = scene
        .player_mover_at(body, 0, [54.565_43, 1.0, 42.147_46], 0)
        .unwrap();
    let pull = Behaviour::MoveTo(MoveTo {
        destination,
        surface,
        free_spot: true,
        speed: Speed::MetresPerSecond(20.0),
        acceleration: 100.0,
        slow_radius: 0.0,
        lateral_seconds: f64::from(0.3_f32),
        stuck_distance: 1.0,
        stuck_seconds: f64::from(0.3_f32),
        exit_horizontal: 10_000.0,
        exit_vertical: 10_000.0,
        exit_total: 10_000.0,
        exit_on_end: false,
        abort_past: true,
    });
    let key = |raw_state| MoveKey {
        instance: 1,
        raw_state,
        activation: 1,
    };
    let command = |frame| Command {
        frame,
        ..Command::default()
    };
    let mut poses = Vec::new();
    let mut frame = 0;
    let mut run = |mover: &mut orpheus_physics::mover::Mover,
                   behaviour: Option<(MoveKey, &Behaviour)>| loop {
        frame += 1;
        let outcome = mover
            .step_pose_to(rays, command(frame), DT, behaviour)
            .unwrap();
        poses.push(mover.pose());
        if let Some(end) = outcome {
            break (end, frame);
        }
    };
    let (end, driven) = run(&mut mover, Some((key(1), &pull)));
    let velocity = mover.pose().velocity;
    let walked = walk.map_or(0, |point| {
        let walk = Behaviour::PathWalk(PathWalk {
            point,
            seconds: f64::from(0.2_f32),
        });
        run(&mut mover, Some((key(30), &walk))).1 - driven
    });
    let mut frame = driven + walked;
    for _ in 0..after {
        frame += 1;
        mover.step_pose_to(rays, command(frame), DT, None).unwrap();
        poses.push(mover.pose());
    }
    Pulled {
        mover,
        driven,
        end,
        velocity,
        walked,
        path: poses.iter().map(|pose| pose.position).collect(),
        poses,
    }
}

/// Each of her client's samples (component 4, the feet plus 1 m, as
/// printed) is a frame of `path`.
fn assert_on_path(path: &[[f64; 3]], samples: &[[f64; 3]]) {
    for sample in samples {
        let feet = [sample[0], sample[1] - 1.0, sample[2]];
        assert!(
            path.iter()
                .any(|at| (0..3).all(|axis| (at[axis] - feet[axis]).abs() < 1e-4)),
            "{feet:?} not on {path:?}"
        );
    }
}

/// Widowmaker's pull at a wall face (`005A` st1 from the Range spawn, look
/// 90° and 20° up), as her client ran it (2026-11-19, owdbg session C:
/// the mover's velocity at each tick, `Grapple.md` "DayGrapple5"). The
/// exit limits are unset (10000, nothing held), so the frame that would
/// land on the target keeps the pull's velocity and runs into the wall,
/// which turns it along the face; the feet end past the target, so the
/// grapple's check after the move blocks it on that frame (slot 18
/// `0x7FF789B573C0`). She leaves the wall rising at 8.8 m/s along it, the
/// wall's top lip and the next wall turn her, and she lands 3.6 m along
/// the wall, where her client rested.
#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn widowmakers_pull_at_a_wall_keeps_its_velocity_and_rests_where_her_client_did() {
    use orpheus_physics::mover::MoveEnd;
    let pulled = pull_from_the_spawn(
        [60.397_644, 4.622_823, 42.147_46],
        [-0.985_831, 0.0, 0.167_742],
        None,
        120,
    );
    // Her client's behaviour drove 19 ticks; the velocity its end (slot 5
    // `0x7FF789B539B0`) found, f32 as recorded.
    assert_eq!((pulled.driven, pulled.end), (19, MoveEnd::Blocked));
    assert_eq!(pulled.velocity, [0.554_687_5, 8.826_171_875, 3.257_812_5]);
    // The movement state her client's tick had on each of those frames
    // (flags 0x18 in session C): the begin took her off the ground and set
    // the air ticks to 0, and only the lateral share counts them while the
    // move drives (no transitions: she neither lands nor counts air ticks
    // otherwise); the move's record holds its phase and the checkpoint,
    // which starts at her feet (the first frame's check counts one frame
    // there). A correction that carries these lets her client's replay go
    // on with the same move instead of beginning it again.
    let spawn = [54.565_429_687_5, 1.0, 42.147_460_937_5];
    let driven = &pulled.poses[..19];
    for (frame, pose) in (1..).zip(driven) {
        assert_eq!(pose.flags & !TOUCHING, FELL | MOVE_TO, "frame {frame}");
        assert_eq!(pose.air_ticks, frame, "frame {frame}");
        let record = pose.move_to.unwrap();
        assert_eq!(
            record.phase,
            if frame < 19 { 1 } else { 2 },
            "frame {frame}"
        );
    }
    let first = driven[0].move_to.unwrap();
    assert_eq!((first.checkpoint, first.still), (spawn, 1));
    // The frame after, the state let it go: the end cleared the flag.
    let after = pulled.poses[19];
    assert_eq!((after.flags & MOVE_TO, after.move_to), (0, None));
    // Past the wall's top lip, and past the next wall.
    assert_on_path(
        &pulled.path,
        &[[59.4990, 6.5518, 43.9248], [59.1377, 5.9805, 44.6191]],
    );
    // Where her client rested (component 4 (58.128906, 2.0, 44.796875)).
    assert_eq!(
        pulled.mover.pose().position,
        [58.128_906_25, 1.0, 44.796_875]
    );
}

/// Widowmaker's pull at a wall's top (`005A` st1 from the Range spawn,
/// yaw 180°, pitch16 −2720), as her client ran it (2026-11-19, owdbg
/// sessions `grapple6a` and `grapple6b`, `Grapple.md` "DayGrapple6"). The
/// hook meets the wall's face below its top; the free spot lies behind a
/// solid lip at z 29.2 that her capsule meets on the way, which turns the
/// pull up and along the lip. She rises to the target's height against it
/// and stays within 1 m of her last checkpoint: 0.3 s later (19 frames of
/// 16 ms) and more than 1 m from the target the move blocks, on the 69th
/// driven frame as the server ran it in `grapple6b`, with the velocity her
/// client's mover had there (its end, slot 5 `0x7FF789B539B0`). Her
/// client's path passes the lip and the top, and she rests where it
/// rested.
#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn widowmakers_pull_at_a_wall_top_holds_against_the_lip_and_rests_where_her_client_did() {
    use orpheus_physics::mover::MoveEnd;
    let pulled = pull_from_the_spawn(
        [54.565_43, 6.425_825, 27.435_92],
        [0.167_743, -3.113_021e-7, 0.985_831],
        None,
        240,
    );
    assert_eq!((pulled.driven, pulled.end), (69, MoveEnd::Blocked));
    assert_eq!(
        pulled.velocity,
        [0.030_273_437_5, 0.024_414_062_5, -0.004_882_812_5]
    );
    // At the lip and held at the top.
    assert_on_path(
        &pulled.path,
        &[[54.7559, 7.2910, 29.6943], [54.9424, 7.7344, 29.6631]],
    );
    assert_eq!(
        pulled.mover.pose().position,
        [55.086_914_062_5, 1.0, 30.386_718_75]
    );
}

/// Widowmaker's grapple to a ledge's face just under its top (`005A` from
/// the Range spawn, yaw −30°, 14° up), then the ledge pull (`005A` case 2,
/// `STU_1F5ED928` st30, mover behaviour kind 10), as her client ran them
/// (2026-11-19, owdbg session `ledge3`, `Grapple.md` "DayLedgePull2"). The
/// pull stops under the lip; the walk starts from her feet there, and her
/// client's path build (`0x7FF789B5EC50`, set-up `0x7FF789B578B0`) found
/// three points, the last in the air over the ledge; with no component 86
/// on her body it cast no ledge probes. Over 0.2 s she walks up along it,
/// each tick's velocity her client's to the bit; the last tick, without
/// forward input, moves at a quarter of the cap (path length / 0.2 s), so
/// she drifts over the top, lands on it and rests where her client rested.
#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn widowmakers_ledge_pull_walks_her_onto_the_ledge_where_her_client_did() {
    use orpheus_physics::mover::MoveEnd;
    use orpheus_wire::entity::NAV_PATH;
    // The hook point (her client's walk block `+0x30`) and its normal.
    let hook = [52.531_166, 3.514_61, 45.671_169];
    let pulled = pull_from_the_spawn(hook, [0.986_533, 0.0, -0.163_562], Some(hook), 60);
    // The velocity her client's kind-1 end (slot 5 `0x7FF789B539B0`)
    // found: the pull blocked under the lip.
    assert_eq!((pulled.driven, pulled.end), (13, MoveEnd::Blocked));
    assert_eq!(pulled.velocity, [0.0, 7.630_859_375, 0.0]);
    // Her feet when the walk began: her client's block `+0x20`.
    let start = pulled.poses[12].position;
    assert_eq!(start, [53.045_898_437_5, 2.596_679_687_5, 45.623_046_875]);
    // The velocity after each tick of the walk (kind 10's slot 11
    // `0x7FF789B53130` read it at the next tick, its end at the last).
    let client = [
        [0.483_398_437_5, 7.932_617_187_5, -2.868_164_062_5],
        [-0.039_062_5, 8.107_421_875, -2.378_906_25],
        [-0.472_656_25, 8.206_054_687_5, -1.954_101_562_5],
        [-0.830_078_125, 8.255_859_375, -1.593_75],
        [-1.119_140_625, 8.273_437_5, -1.294_921_875],
        [-1.362_304_687_5, 8.271_484_375, -1.051_757_812_5],
        [-1.587_890_625, 8.268_554_687_5, -0.700_195_312_5],
        [-1.829_101_562_5, 8.248_046_875, -0.043_945_312_5],
        [-2.019_531_25, 8.188_476_562_5, 0.506_835_937_5],
        [-2.168_945_312_5, 8.110_351_562_5, 0.953_125],
        [-2.279_296_875, 8.030_273_437_5, 1.304_687_5],
        [-2.374_023_437_5, 7.950_195_312_5, 1.594_726_562_5],
        [-0.602_539_062_5, 1.978_515_625, 0.430_664_062_5],
    ];
    assert_eq!(pulled.walked, 13);
    let walk = &pulled.poses[13..26];
    for (tick, (pose, velocity)) in walk.iter().zip(client).enumerate() {
        assert_eq!(pose.velocity, velocity, "tick {tick}");
        assert_eq!(pose.flags & !TOUCHING, FELL | NAV_PATH, "tick {tick}");
        let record = pose.nav_path.unwrap();
        assert_eq!(record.state, if tick < 12 { 1 } else { 2 }, "tick {tick}");
    }
    // Her client's polyline: the lifted start, a turn, and a point in the
    // air over the ledge (no ledge probe point); the cap is the path's
    // length over 0.2 s, 8.448872 on her client (an ulp apart: the
    // `rsqrtss` estimate in the lengths differs between CPUs).
    let record = walk[0].nav_path.unwrap();
    assert_eq!(
        &record.points[..usize::from(record.count)],
        [
            [53.125, 3.152_681_6, 45.375],
            [52.875, 4.023_315_4, 45.375],
            [52.625, 4.723_022_5, 45.625],
        ]
    );
    assert!((record.cap - 8.448_872).abs() <= 1e-6, "{}", record.cap);
    // The frame after, the state let it go: the end cleared the flag.
    let after = pulled.poses[26];
    assert_eq!((after.flags & NAV_PATH, after.nav_path), (0, None));
    // Her client's samples: up the walk, over the top, touching down.
    assert_on_path(
        &pulled.path,
        &[
            [53.0312, 4.1162, 45.4824],
            [52.9053, 4.7764, 45.4414],
            [52.7275, 5.2891, 45.5508],
            [52.6006, 4.9434, 45.6396],
        ],
    );
    // Where her client rested (component 4 (52.6006, 4.9492, 45.6396)).
    assert_eq!(
        pulled.mover.pose().position,
        [52.600_585_937_5, 3.949_218_75, 45.639_648_437_5]
    );
}

/// Pharah's Jump Jet (graph `0027` node 1, `STU_FF3471E9`): the vertical
/// velocity set to 0 and `(sin yaw, 2, cos yaw) * 10` added for one pose
/// lift her off the ground well above a jump's 1 m, forward along her yaw;
/// she comes down and rests (from the spawn facing +z she lands on a ledge
/// 6 m higher).
fn jump_jet_lifts_her_off_the_ground(mut pharah: Player) {
    let start = pharah.position();
    pharah.press(BLINK, 0);
    let mut top = start[1];
    for _ in 0..60 {
        pharah.idle(1, 0);
        top = top.max(pharah.position()[1]);
    }
    assert!(top - start[1] > 2.0, "{start:?} rose to {top}");
    pharah.idle(300, 0);
    let end = pharah.position();
    assert!(
        end[1] < top && end[2] > start[2],
        "{start:?} -> top {top} -> {end:?}"
    );
    pharah.idle(30, 0);
    assert_eq!(pharah.position(), end);
}

#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn pharahs_jump_jet_lifts_her_off_the_ground() {
    jump_jet_lifts_her_off_the_ground(Player::spawn(PHARAH));
}

/// An AI hero's body (an AI entity possesses it) takes its graphs'
/// velocity actions through the same mover path as a player's.
#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn an_ai_pharahs_jump_jet_lifts_her_off_the_ground() {
    let pharah = Player::spawn_ai(PHARAH);
    assert!(pharah.world.is_headless());
    let graphs = pharah.scene.graphs();
    assert!(
        graphs.ai_definitions().definition(PHARAH).is_some(),
        "her AI entity possesses her body"
    );
    jump_jet_lifts_her_off_the_ground(pharah);
}

/// Echo's body graph `1DFC` keeps a RayCast (st10) 2.5 m down from a
/// quarter metre over her feet against the map (ray profile `002C.097`),
/// its hit into #2413, which her glide waits to be false: standing at the
/// spawn, it meets the ground.
#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn echos_ground_ray_meets_the_floor_she_stands_on() {
    let echo = Player::spawn(ECHO);
    let snapshot = echo.snapshot();
    let (_, body) = snapshot
        .instances
        .iter()
        .find(|(_, instance)| instance.graph == 0x1dfc)
        .unwrap();
    assert_eq!(body.variables.get(&2413), Some(&ScriptValue::Bool(true)));
    assert!(
        !snapshot
            .assumptions
            .iter()
            .any(|item| item.answer.starts_with("casts nothing")),
        "{:?}",
        snapshot.assumptions
    );
}

const DOOMFIST: u64 = 0x02e0_0000_0000_012f;

impl Player {
    /// Whether the owner gets his body's records now, and whether a pose
    /// of the last frame waits to go to him.
    fn records(&self) -> (bool, bool) {
        let script = self.world.body_script.as_ref().unwrap();
        let history = script.corrections.as_ref().unwrap();
        (
            script.runtime.owner_needs_records(),
            history.candidate(self.frame, true).is_some(),
        )
    }

    /// Graph `0D06`'s state `raw` (Rocket Punch) is on.
    fn punch_state(&self, raw: u16) -> bool {
        self.snapshot()
            .instances
            .values()
            .filter(|instance| instance.graph == 0x0d06)
            .any(|instance| instance.states.get(&raw).is_some_and(|state| state.active))
    }
}

/// Rocket Punch (graph `0D06`) on the server: the charge (st11) while
/// secondary fire is held, then at the release the dash, a server-only
/// MovementMod (st42: forward throttle 1 at 8.18 times the run speed,
/// 45 m/s) along the facing at the release (`STU_09F9B1FE`, #2083) however
/// the view turns, with the registration st67 (`STU_4773DA56`) on. His
/// client predicts none of the dash and draws his view from the records
/// meanwhile, so he gets them from the charge to 250 ms after the punch,
/// and not before or after.
#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn doomfists_rocket_punch_dashes_along_its_release_facing_with_records_to_its_owner() {
    let mut doomfist = Player::spawn(DOOMFIST);
    let (open, _) = open_and_walled(&doomfist.scene);
    assert_eq!(doomfist.records(), (false, false));
    let hold = |doomfist: &mut Player, buttons, yaw| {
        doomfist.step(Command {
            yaw,
            buttons,
            ..Command::default()
        });
    };
    // Half a second of charge (more than its least 0.2 s, #9273).
    for _ in 0..32 {
        hold(&mut doomfist, orpheus_wire::commands::SECONDARY, open);
        assert!(doomfist.punch_state(11), "charging");
        assert_eq!(doomfist.records(), (true, true));
    }
    let start = doomfist.position();
    // Released, then the view turns a quarter turn at once.
    let turned = open + 16384;
    hold(&mut doomfist, 0, open);
    assert!(
        doomfist.punch_state(42) && doomfist.punch_state(67),
        "dashing"
    );
    for _ in 0..6 {
        hold(&mut doomfist, 0, turned);
        assert_eq!(doomfist.records(), (true, true));
    }
    let moved = horizontal(start, doomfist.position());
    // 6 frames of the dash at up to 45 m/s (its first frame accelerates).
    assert!(moved > 3.0 && moved <= 6.0 * 45.0 * DT + 0.01, "{moved}");
    let [dx, _, dz] = std::array::from_fn(|axis| doomfist.position()[axis] - start[axis]);
    let facing = f64::from(open) * std::f64::consts::TAU / 65536.0;
    let off = (dx.atan2(dz) - facing + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
        - std::f64::consts::PI;
    assert!(off.abs() < 0.01, "the dash turned by {off} rad");
    let mut last = None;
    for _ in 0..90 {
        hold(&mut doomfist, 0, turned);
        if doomfist.records().0 {
            last = Some(doomfist.frame);
        }
    }
    let last = last.expect("records through the dash");
    assert!(
        !doomfist.punch_state(42) && !doomfist.punch_state(11),
        "the punch ended"
    );
    // The window closed 250 ms (16 frames) after the last frame that needed
    // records: no pose waits for him.
    assert!(doomfist.frame > last + 16);
    assert_eq!(doomfist.records(), (false, false));
}

/// A Rocket Punch into a wall ends there: the wall check (`0D06` st7,
/// `STU_FBFF5DE7`) finds the trial move ahead stopped once the dash meets
/// the wall, its server-only rays (st79/st80) hit, and the dash (st42)
/// gives way to the impact (st9) at the frame the body stopped, where it
/// stays.
#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn doomfists_rocket_punch_ends_at_a_wall_with_the_impact() {
    let mut doomfist = Player::spawn(DOOMFIST);
    let (_, walled) = open_and_walled(&doomfist.scene);
    let hold = |doomfist: &mut Player, buttons| {
        doomfist.step(Command {
            yaw: walled,
            buttons,
            ..Command::default()
        });
    };
    // A full charge (1.4 s): the dash would go well past the wall.
    for _ in 0..90 {
        hold(&mut doomfist, orpheus_wire::commands::SECONDARY);
    }
    let start = doomfist.position();
    hold(&mut doomfist, 0);
    assert!(doomfist.punch_state(42), "dashing");
    let mut stopped = None;
    let mut impact = None;
    let mut last = doomfist.position();
    for frame in 0..60 {
        hold(&mut doomfist, 0);
        let now = doomfist.position();
        if stopped.is_none() && frame > 0 && horizontal(last, now) < 0.3 {
            stopped = Some(frame);
        }
        if impact.is_none() && doomfist.punch_state(9) {
            impact = Some(frame);
            assert!(!doomfist.punch_state(42), "the dash ended with the impact");
        }
        last = now;
    }
    let (stopped, impact) = (
        stopped.expect("the wall stops him"),
        impact.expect("an impact"),
    );
    assert!(
        impact <= stopped + 1,
        "impact at {impact}, stopped at {stopped}"
    );
    let moved = horizontal(start, doomfist.position());
    assert!(moved < 6.0, "{moved}");
}
