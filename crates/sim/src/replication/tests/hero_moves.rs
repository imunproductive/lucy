//! Heroes' moves through the player's tick: the body script on the derived
//! Practice Range scene, with its collision. Tracer's Blink and Recall;
//! Pharah's Jump Jet; Echo's ground ray. Tracer's graphs still reach a config class the VM
//! doesn't compute (`STU_038C32E8`) and `StateBlinkEffect`, neither of
//! which decides the moves tested here, so the body tolerates unknowns as
//! release builds do.

use super::*;
use orpheus_script::{
    snapshot::{ProjectedSnapshot, View},
    stream::{BodyStream, ChangeKey, DeltaKind, OWNER_SETTLE_FRAMES},
    vm::{State, StateKind, Unknowns},
    wire::Value as ScriptValue,
};
use orpheus_wire::{
    commands::{Command, JUMP},
    entity::MOVE_TO,
};
use std::collections::BTreeSet;

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
        Self::spawn_on(hero, scene, world)
    }

    /// `hero` picked on Hanamura's Assault (C32S2's map, the install's
    /// collision), his feet put at `feet` facing `yaw`, standing for a
    /// second.
    fn on_hanamura(hero: u64, feet: [f64; 3], yaw: i32) -> Self {
        const HANAMURA: u64 = 0x0800_0000_0000_0165;
        let runtime = orpheus_testkit::runtime_dir();
        let mut assets = SceneAssets::load(&runtime).unwrap().with_collision_build(
            crate::scene_assets::install_collision_build(orpheus_testkit::game_dir()),
        );
        assets.set_player_unknowns(Unknowns::Tolerate);
        let map = orpheus_content::maps::MapCatalog::load(runtime.join("data"))
            .unwrap()
            .game_map(
                HANAMURA,
                crate::match_flow::ASSAULT,
                "Assault",
                false,
                &[6, 6],
            )
            .unwrap();
        let scene = assets.prepare(map).unwrap();
        let mut player = Self::spawn_on(hero, scene, WorldDelivery::new(OWNER).unbuffered());
        let body = player.scene.heroes().get(hero).unwrap().body;
        player.world.mover = Some(player.scene.player_mover_at(body, 0, feet, yaw).unwrap());
        player.idle(60, yaw);
        player
    }

    fn spawn_on(hero: u64, scene: PreparedScene, world: WorldDelivery) -> Self {
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
/// its hit bound to #2413, which her glide waits to be false: standing at
/// the spawn, it meets the ground.
#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn echos_ground_ray_meets_the_floor_she_stands_on() {
    let echo = Player::spawn(ECHO);
    let snapshot = echo.snapshot();
    let (&body, _) = snapshot
        .instances
        .iter()
        .find(|(_, instance)| instance.graph == 0x1dfc)
        .unwrap();
    assert_eq!(
        snapshot.resolved_bindings.get(&(body, 2413)),
        Some(&ScriptValue::Bool(true))
    );
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
const ABILITY_1: u32 = 0x08;
const ABILITY_2: u32 = 0x10;

/// The angle (radians, -π..π) from the heading `yaw` to the flat move
/// (`dx`, `dz`).
fn off_heading(dx: f64, dz: f64, yaw: i32) -> f64 {
    let facing = f64::from(yaw) * std::f64::consts::TAU / 65536.0;
    (dx.atan2(dz) - facing + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
        - std::f64::consts::PI
}

impl Player {
    /// Whether a pose of the last frame waits to go to the owner.
    fn pose_waits(&self) -> bool {
        let script = self.world.body_script.as_ref().unwrap();
        let history = script.corrections.as_ref().unwrap();
        history.candidate(self.frame, true).is_some()
    }

    /// Graph `0D06`'s state `raw` (Rocket Punch) is on.
    fn punch_state(&self, raw: u16) -> bool {
        self.graph_state(0x0d06, raw)
    }

    /// State `raw` of an instance of `graph` is on.
    fn graph_state(&self, graph: u32, raw: u16) -> bool {
        self.snapshot()
            .instances
            .values()
            .filter(|instance| instance.graph == graph)
            .any(|instance| instance.states.get(&raw).is_some_and(|state| state.active))
    }

    /// How many times state `raw` of the first instance of `graph` began.
    fn graph_activation(&self, graph: u32, raw: u16) -> u16 {
        self.snapshot()
            .instances
            .values()
            .find(|instance| instance.graph == graph)
            .and_then(|instance| instance.states.get(&raw))
            .map_or(0, |state| state.activation)
    }

    /// Instance variable `var` of the first instance of `graph`.
    fn graph_variable(&self, graph: u32, var: u32) -> Option<&ScriptValue> {
        self.snapshot()
            .instances
            .values()
            .find(|instance| instance.graph == graph)?
            .variables
            .get(&var)
    }

    fn velocity(&self) -> [f64; 3] {
        self.world.mover.as_ref().unwrap().pose().velocity
    }

    fn grounded(&self) -> bool {
        self.world.mover.as_ref().unwrap().pose().flags & FELL == 0
    }

    /// The id of the first instance of `graph`.
    fn instance_of(&self, graph: u32) -> u16 {
        *self
            .snapshot()
            .instances
            .iter()
            .find(|(_, instance)| instance.graph == graph)
            .unwrap()
            .0
    }

    /// His statescript as his owner frames project it now.
    fn owner_projection(&self) -> ProjectedSnapshot {
        let runtime = &self.world.body_script.as_ref().unwrap().runtime;
        runtime
            .encoder()
            .project(runtime.snapshot().unwrap(), View::Owner)
            .unwrap()
    }
}

/// His owner frames as `queue_delta` makes them, each one acknowledged at
/// once.
struct OwnerChunks(BodyStream<ProjectedSnapshot>);

impl OwnerChunks {
    /// The full owner frame and the initial delta, acknowledged.
    fn start(player: &Player) -> Self {
        let runtime = &player.world.body_script.as_ref().unwrap().runtime;
        let encoder = runtime.encoder();
        let mut stream = BodyStream::new(OWNER | 1);
        stream.create_arrived();
        let full = encoder
            .queue_full(&mut stream, &player.owner_projection())
            .unwrap();
        stream.arrived(full.last);
        let mut chunks = Self(stream);
        chunks.queue(player, DeltaKind::InitialOwner(player.frame));
        chunks
    }

    /// The keys of the correction his last frame makes go; none when
    /// nothing his client holds changed.
    fn correction(&mut self, player: &Player) -> Option<BTreeSet<ChangeKey>> {
        self.queue(
            player,
            DeltaKind::Correction(player.frame + OWNER_SETTLE_FRAMES),
        )
    }

    fn queue(&mut self, player: &Player, kind: DeltaKind) -> Option<BTreeSet<ChangeKey>> {
        let runtime = &player.world.body_script.as_ref().unwrap().runtime;
        let chunk = runtime
            .encoder()
            .queue_delta(
                &mut self.0,
                &player.owner_projection(),
                kind,
                runtime.frame_ms(),
            )
            .unwrap()?;
        let keys = self.0.sent(chunk.last).unwrap().changes.clone();
        assert!(self.0.arrived(chunk.last).advanced);
        Some(keys)
    }
}

/// C32S2's checkpoint 10 on Hanamura as game session s3 played it
/// (`D:/Temp/df7/s3/cp10_slam`, `refs/reports/Doomfist.md` §14-15): from his
/// feet at (5.106, 7.810, 7.769) facing 80.7° (view 14692) and looking 15°
/// down (2731), a jump walking forward for 19 frames (7512-7530), 13
/// frames of nothing, then E for one frame (7544). His ground ray (`0B27`
/// st8) misses from 7534, so st7 begins the landing search st6 (with st5
/// and st66, the preview). The press (st63 sets `#1321`) opens the search's
/// gate; it finds the point and the transition `#1627 && #1321` begins the
/// flight (st31, st9) in the press's own frame, and st31's begin aborts the
/// button state st64 before the release (7545) can stop the ability. His
/// owner frames carry the ray and the search: left out, his client never
/// ran them, waited a round trip for the server's point, and its st64
/// stopped the tapped slam (its flight showed 3 frames late, after the
/// server's chunk rolled it back). The flight's chase of `#10472e` (st17,
/// toward the variable `#10471e`, so it steps each frame on both sides)
/// makes no correction go: in s3 one went every frame of the flight.
#[test]
#[ignore = "requires the derived 104319 data and the install (Hanamura's collision)"]
fn doomfists_tapped_airborne_slam_flies_from_the_press_frame() {
    const FEET: [f64; 3] = [5.106_445_312_5, 7.809_570_312_5, 7.768_554_687_5];
    let view = Command {
        yaw: 14692,
        pitch: 2731,
        ..Command::default()
    };
    let mut doomfist = Player::on_hanamura(DOOMFIST, FEET, view.yaw);
    assert_eq!(doomfist.position(), FEET);
    // s3's server poses (`orpheus::pose`, frames 7512 on); his client's
    // own prediction matched them to the millimetre until the press.
    for frame in 7512..=7531 {
        doomfist.step(Command {
            forward: if frame > 7512 { 127 } else { 0 },
            buttons: if frame < 7531 { JUMP } else { 0 },
            ..view
        });
    }
    let mut preview = None;
    for frame in 7532..=7543 {
        doomfist.step(view);
        if preview.is_none() && doomfist.graph_state(0x0b27, 6) {
            preview = Some(frame);
        }
    }
    assert_eq!(preview, Some(7534));
    assert_eq!(
        doomfist.position(),
        [7.032_226_562_5, 8.515_625, 8.083_007_812_5]
    );
    let slam = doomfist.instance_of(0x0b27);
    let owner = doomfist.owner_projection();
    assert!(
        owner.state(slam, 6).is_some(),
        "the landing search goes out"
    );
    assert!(owner.state(slam, 8).is_some(), "the ground ray goes out");
    let mut chunks = OwnerChunks::start(&doomfist);
    // 7544: the press begins the flight.
    doomfist.step(Command {
        buttons: ABILITY_2,
        ..view
    });
    assert!(doomfist.graph_state(0x0b27, 9), "the flight begins");
    assert!(!doomfist.graph_state(0x0b27, 64), "st31 aborted st64");
    assert!(doomfist.graph_state(0x0b27, 17), "the chase runs");
    // The point his client's own search found in df8 s1 (#431 at its
    // press frame 7088, `D:/Temp/df8/s1/cp10_slam.jsonl`; the same jump and
    // view), to its f32 step (9.5e-7 at 10): the server's was 1.7 m short,
    // (18.556, 4.143, 9.973), before it searched as the client does.
    let Some(ScriptValue::Vector(point)) = doomfist.graph_variable(0x0b27, 431) else {
        panic!("no landing point");
    };
    let client = [
        20.236_936_569_213_867,
        4.176_490_306_854_248,
        10.249_571_800_231_934,
    ];
    assert!(
        point
            .iter()
            .zip(client)
            .all(|(a, b)| (a - b).abs() <= 9.6e-7),
        "{point:?}"
    );
    let flight = chunks.correction(&doomfist).expect("the press's chunk");
    assert!(flight.contains(&ChangeKey::State(slam, 9)), "{flight:?}");
    // 7545: his first move toward the point, as his client made it (7089).
    doomfist.step(view);
    assert_eq!(
        doomfist.velocity(),
        [23.479_492_187_5, -7.672_851_562_5, 3.852_539_062_5]
    );
    assert_eq!(
        doomfist.position(),
        [7.493_164_062_5, 8.340_820_312_5, 8.158_203_125]
    );
    // The flight's other changes: 0.15 s after the search ended (st65,
    // 120 704 + 150 ms, in frame 7553) the preview's entity st5 ends (his
    // client's st5/st65 ended at 7097, the same frame), and at 7558 the
    // ground ray (3 m down from 0.25 m above his feet) meets the ground
    // (#2413) on the way to his client's point. The ray's hit is bound to
    // the ray (slot 0) as his client binds it, so his client casts it
    // itself and no correction goes for it: one that listed #2413 without
    // the binding unbound it there, and from then on only the server's
    // frames changed it, as restores that wake no watcher (df10 s2
    // cp4_bufslam: `0B27` st7 never watched `#10473e`, so a slam pressed
    // during Rising Uppercut flew 2 frames late on his client).
    let ray_hit = |doomfist: &Player| {
        doomfist
            .snapshot()
            .resolved_bindings
            .get(&(slam, 2413))
            .cloned()
    };
    let bound: Vec<_> = doomfist.snapshot().bindings[&(slam, 2413)]
        .iter()
        .map(|binding| (binding.raw_state, binding.slot))
        .collect();
    assert_eq!(bound, [(8, 0)]);
    let mut corrections = Vec::new();
    let mut hit = None;
    for frame in 7545..7560 {
        assert!(doomfist.graph_state(0x0b27, 17), "frame {frame}");
        if let Some(keys) = chunks.correction(&doomfist) {
            corrections.push((frame, keys));
        }
        if hit.is_none() && ray_hit(&doomfist) == Some(ScriptValue::Bool(true)) {
            hit = Some(frame);
        }
        doomfist.step(view);
    }
    let [(7553, ended)] = &corrections[..] else {
        panic!("{corrections:?}");
    };
    assert!(
        ended.contains(&ChangeKey::State(slam, 5)) && ended.contains(&ChangeKey::State(slam, 65)),
        "{ended:?}"
    );
    assert_eq!(hit, Some(7558));
}

/// Rocket Punch (graph `0D06`) on the server: the charge (st11) while
/// secondary fire is held, then at the release the dash, a server-only
/// MovementMod (st42: forward throttle 1 at 8.18 times the run speed,
/// 45 m/s) along the facing at the release (`STU_09F9B1FE`, #2083) however
/// the view turns, with the registration st67 (`STU_4773DA56`) on. His
/// client predicts none of the dash and draws his view from the records
/// meanwhile; Doomfist is corrected (`CORRECTED_HEROES`), so his owner gets
/// his body's pose at every frame: before the charge, through the charge
/// and the dash, and after the punch.
#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn doomfists_rocket_punch_dashes_along_its_release_facing_with_each_pose_to_its_owner() {
    let mut doomfist = Player::spawn(DOOMFIST);
    let (open, _) = open_and_walled(&doomfist.scene);
    assert!(doomfist.pose_waits());
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
        assert!(doomfist.pose_waits());
    }
    let start = doomfist.position();
    // Released, then the view turns a quarter turn at once.
    let turned = open + 16384;
    hold(&mut doomfist, 0, open);
    assert!(
        doomfist.punch_state(42) && doomfist.punch_state(67),
        "dashing"
    );
    // The dash's view limits (st45, `STU_9935FBC8`): ±60° of yaw and ±30°
    // of pitch around his facing at the release (#501, #502, degrees),
    // each × 0.017453292 as his client's `0x7FF789BBC810` keeps them.
    let punch = doomfist.instance_of(0x0d06);
    let degrees = |var| match doomfist.graph_variable(0x0d06, var) {
        Some(ScriptValue::Float(value)) => *value as f32,
        other => panic!("#{var}: {other:?}"),
    };
    let facing = [degrees(501), degrees(502), -60.0, -30.0, 60.0, 30.0];
    let expected: Vec<_> = facing
        .iter()
        .map(|degrees| ScriptValue::Float((degrees * 0.017_453_292).into()))
        .chain([ScriptValue::Float(0.0)])
        .collect();
    assert_eq!(
        doomfist
            .owner_projection()
            .state_payload(punch, 45)
            .map(|payload| &payload["values"]),
        Some(&ScriptValue::Array(expected))
    );
    for _ in 0..6 {
        hold(&mut doomfist, 0, turned);
        assert!(doomfist.pose_waits());
    }
    let moved = horizontal(start, doomfist.position());
    // Game session s3 (D:/Temp/df5, `refs/reports/Doomfist.md` §12), the
    // punch released at frame 11967: its first dash frame moved 0.4033 m
    // ((-4.246, -24.840) m/s), each later one 0.7202 m (45.0 m/s, the
    // (-0.1211, -0.7100) m steps of the 1/1024 m grid): 4.0044 m in all
    // after 6 frames. Here the heading differs, and each frame's feet snap
    // to the grid on each axis (up to 0.49 mm), so 6 frames may differ by
    // up to 6 × 0.69 mm.
    assert!((moved - 4.0044).abs() < 0.0042, "{moved}");
    let [dx, _, dz] = std::array::from_fn(|axis| doomfist.position()[axis] - start[axis]);
    let off = off_heading(dx, dz, open);
    assert!(off.abs() < 0.01, "the dash turned by {off} rad");
    for _ in 0..90 {
        hold(&mut doomfist, 0, turned);
        assert!(doomfist.pose_waits());
    }
    assert!(
        !doomfist.punch_state(42) && !doomfist.punch_state(11),
        "the punch ended"
    );
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

/// Reinhardt's Charge (graph `007D`) crashes into the wall ahead where his
/// client's does: the wall check st3 (`STU_FBFF5DE7` in its velocity form,
/// 4 steps, 45°) writes #559 once the trial move ahead is stopped, and the
/// crash (st4) ends the 16.5 m/s charge. Game session r1 (Practice Range
/// spawn, `refs/reports/Doomfist.md` §23), his client's feet equal to the
/// server's at every frame: Shift for 3 frames at 4492, the charge (st2)
/// from 4529, at yaw 0 the crash at 4534 with his feet at z 43.2197265625,
/// resting at 43.283203125 after one slower frame; then at yaw 90 from
/// there, Shift at 5260, the charge at 5297, the crash at 5314 at x
/// 58.8017578125, resting at 58.865234375.
#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn reinhardts_charge_crashes_into_the_wall_where_his_client_did() {
    const REINHARDT: u64 = 0x02e0_0000_0000_0007;
    let mut reinhardt = Player::spawn(REINHARDT);
    assert_eq!(reinhardt.position(), [54.5654296875, 1.0, 42.1474609375]);
    // From the press: the charge 37 frames later, the crash `crash` after it.
    let charge = |reinhardt: &mut Player, yaw: i32, crash: u32| {
        for _ in 0..3 {
            reinhardt.step(Command {
                yaw,
                buttons: ABILITY_1,
                ..Command::default()
            });
        }
        reinhardt.idle(34, yaw);
        assert!(!reinhardt.graph_state(0x007d, 2), "the charge's wind-up");
        reinhardt.idle(1, yaw);
        assert!(reinhardt.graph_state(0x007d, 2), "charging");
        reinhardt.idle(crash - 1, yaw);
        assert!(!reinhardt.graph_state(0x007d, 4), "no crash yet");
        reinhardt.idle(1, yaw);
        assert!(reinhardt.graph_state(0x007d, 4), "the crash");
        let crashed = reinhardt.position();
        reinhardt.idle(2, yaw);
        (crashed, reinhardt.position())
    };
    let (crashed, rest) = charge(&mut reinhardt, 0, 5);
    assert_eq!(crashed, [54.5654296875, 1.0, 43.2197265625]);
    assert_eq!(rest, [54.5654296875, 1.0, 43.283203125]);
    // His Charge's cooldown runs out before the next press (12 s apart).
    reinhardt.idle(720, 16384);
    let (crashed, rest) = charge(&mut reinhardt, 16384, 17);
    assert_eq!(crashed, [58.8017578125, 1.0, 43.283203125]);
    assert_eq!(rest, [58.865234375, 1.0, 43.283203125]);
}

/// Seismic Slam on the ground (graph `0B27`, ABILITY_2): the leap (`rid67`
/// on channel 1: `STU_0762B33A` of the facing, here the flat facing raised
/// by 0.3, times 19 m/s) under 1.5 × gravity (st1), the landing (#59 = 3,
/// st58) and 0.1 s after it the end of st37 (`STU_54FEB21C`, a wait the
/// server ends) that brings the shockwave (st44) and the end of the ability,
/// so Rising Uppercut lifts him after it (before, the slam never ended and
/// blocked his other abilities).
#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn doomfists_grounded_slam_leaps_lands_and_ends() {
    let mut doomfist = Player::spawn(DOOMFIST);
    let (open, _) = open_and_walled(&doomfist.scene);
    let start = doomfist.position();
    // The press, then the leap's first frame.
    doomfist.press(ABILITY_2, open);
    let [vx, vy, vz] = doomfist.velocity();
    let flat = vx.hypot(vz);
    assert!((flat.hypot(vy) - 19.0).abs() < 0.01, "{vx} {vy} {vz}");
    assert!((vy / flat - 0.3).abs() < 1e-3, "{vx} {vy} {vz}");
    assert!(off_heading(vx, vz, open).abs() < 0.01, "{vx} {vz}");
    let mut landed = None;
    let mut shockwave = None;
    let mut moved = 0.0;
    for frame in 1..=64 {
        doomfist.idle(1, open);
        if landed.is_none() && doomfist.grounded() {
            landed = Some(frame);
            moved = horizontal(start, doomfist.position());
        }
        if shockwave.is_none() && doomfist.graph_activation(0x0b27, 44) > 0 {
            // It finishes in the frame it begins in, as on his client.
            assert!(!doomfist.graph_state(0x0b27, 44));
            shockwave = Some(frame);
        }
    }
    // Game session s4 (D:/Temp/df5, `refs/reports/Doomfist.md` §12; the
    // client's state after each command tick, equal to the server's to the
    // millimetre): the press at frame 7819 from (54.580, 1.0, 42.235)
    // facing 157.5°, the leap's first frame 7820 (`press` runs both, so the
    // loop's frame k is 7820 + k), the landing at 7847 at (57.292, 1.010,
    // 35.683), 7.0918 m away on the same flat floor, and the shockwave
    // (st44) begun at 7855 (by the server; his client never shows st44 at
    // the end of a frame: s4's slams, df10 s1 cp10_slam).
    let landed = landed.expect("he lands");
    assert_eq!(landed, 27);
    assert!((moved - 7.0918).abs() < 0.002, "{moved}");
    assert_eq!(shockwave, Some(landed + 8));
    doomfist.press(ABILITY_1, open);
    let rises: Vec<f64> = (0..20)
        .map(|_| {
            doomfist.idle(1, open);
            doomfist.velocity()[1]
        })
        .collect();
    // The game (s4, frames 9790-9796): 40 m/s up (`rid8`), less 0.28 m/s of
    // gravity a frame for the 0.1 s of st11, then a fifth of it (`rid14`).
    let rise = rises
        .iter()
        .position(|&vy| vy == 40.0)
        .unwrap_or_else(|| panic!("Rising Uppercut after the slam: {rises:?}"));
    let expected = [40.0, 39.72, 39.44, 39.16, 38.88, 38.60, 7.66];
    for (got, want) in rises[rise..].iter().zip(expected) {
        assert!((got - want).abs() < 0.005, "{:?}", &rises[rise..]);
    }
}

/// Seismic Slam onto the crates by the Range's Training Bot lands where his
/// client did. Game session u2 (`refs/reports/Doomfist.md` §23; the run
/// DoomfistMoves14 saw end 1.26 m off had a 0.8 s client stall in its
/// leap): from (48.9921875, 1.0, 20.1044921875) facing -30183, E at 4678,
/// the leap's first frame at (48.9208984375, 1.0869140625, 19.822265625)
/// moving (-4.4638671875, 5.4599609375, -17.642578125), over a crate's
/// edge onto the crate top, at rest at (45.630859375, 2.0, 12.806640625)
/// on both sides (his client one frame behind from 4694 to the landing,
/// a frame his client skipped, then equal).
#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn doomfists_slam_onto_the_range_crates_rests_where_his_client_did() {
    const YAW: i32 = -30183;
    let mut doomfist = Player::spawn(DOOMFIST);
    let body = doomfist.scene.heroes().get(DOOMFIST).unwrap().body;
    let feet = [48.9921875, 1.0, 20.1044921875];
    doomfist.world.mover = Some(doomfist.scene.player_mover_at(body, 0, feet, YAW).unwrap());
    doomfist.idle(60, YAW);
    let slam = |doomfist: &mut Player| {
        doomfist.step(Command {
            yaw: YAW,
            buttons: ABILITY_2,
            ..Command::default()
        });
    };
    slam(&mut doomfist);
    assert_eq!(doomfist.position(), feet);
    slam(&mut doomfist);
    assert_eq!(
        doomfist.position(),
        [48.9208984375, 1.0869140625, 19.822265625]
    );
    assert_eq!(
        doomfist.velocity(),
        [-4.4638671875, 5.4599609375, -17.642578125]
    );
    slam(&mut doomfist);
    doomfist.idle(60, YAW);
    assert_eq!(doomfist.position(), [45.630859375, 2.0, 12.806640625]);
}

/// Seismic Slam's shockwave (`0B27` st44, `vm/shockwave.rs`): at the
/// landing the wave hits, once, the enemies within its 8 m (`#422`) in the
/// 60° cone along his heading (`#501`) that its sight line from 1.8 m over
/// the landing sees and its walk over the ground reaches; the hits go to
/// `#375` and st42's ModifyHealth strikes them for `#476` from Doomfist.
/// An enemy behind him, one out of reach and an ally ahead are not hit.
#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn doomfists_slam_shockwave_strikes_the_enemies_ahead_of_its_landing() {
    use crate::combat::PosedShot;
    use orpheus_script::vm::TargetCandidate;

    const ENEMY: u32 = 0xa000_0201;
    let mut doomfist = Player::spawn(DOOMFIST);
    let (open, _) = open_and_walled(&doomfist.scene);
    let start = doomfist.position();
    let heading = f64::from(open) * std::f64::consts::TAU / 65536.0;
    let (ahead, side) = (
        [heading.sin(), heading.cos()],
        [heading.cos(), -heading.sin()],
    );
    // The leap lands 7.0918 m along the heading on this floor
    // (`doomfists_grounded_slam_leaps_lands_and_ends`).
    let at = |forward: f64, across: f64| {
        let along = 7.0918 + forward;
        [
            start[0] + ahead[0] * along + side[0] * across,
            start[1],
            start[2] + ahead[1] * along + side[1] * across,
        ]
    };
    let body = |entity, team, feet| TargetCandidate {
        entity,
        team: Some(team),
        dead: false,
        controller: None,
        player: None,
        ai: false,
        ai_control: None,
        feet,
        height: 1.8,
        radius: 0.4,
        yaw: 0.0,
        tags: None,
        health: None,
        spawned: Vec::new(),
        hardpoints: None,
    };
    let candidates = [
        body(OWNER | 1, 0, start),
        // 4 m ahead and 1 m aside: in the cone.
        body(ENEMY, 1, at(4.0, 1.0)),
        // Behind and aside, away from the leap's path.
        body(ENEMY + 1, 1, at(-3.0, 3.0)),
        // Ahead, past the 8 m.
        body(ENEMY + 2, 1, at(9.0, 0.0)),
        body(ENEMY + 3, 0, at(3.0, -1.0)),
    ];
    doomfist.world.set_target_candidates(&candidates);
    doomfist.press(ABILITY_2, open);
    let mut struck = Vec::new();
    let mut damage = None;
    for _ in 0..64 {
        doomfist.idle(1, open);
        for shot in doomfist.world.take_shots() {
            if let PosedShot::Health(change) = shot {
                struck.push((change.target, change.instigator, change.amount, change.heal));
            }
        }
        if doomfist.graph_activation(0x0b27, 44) > 0 && damage.is_none() {
            damage = doomfist.graph_variable(0x0b27, 476).cloned();
        }
    }
    let Some(ScriptValue::Float(damage)) = damage else {
        panic!("the slam's damage #476: {damage:?}");
    };
    assert!(damage > 0.0, "{damage}");
    assert_eq!(struck, [(ENEMY, OWNER | 1, damage, false)]);
}

/// The punch jump: a jump pressed at least 0.1 s into Rocket Punch's dash
/// (st63, then the jump button st62 sets #11432) ends the dash (st5) and
/// launches him (`rid112` adds the facing × 25 and 5 up on channel 1 to
/// what the dash leaves him). All of it is server-only (st42's end plug),
/// so his client shows it from the records.
#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn doomfists_punch_jump_launches_him_out_of_the_dash() {
    let mut doomfist = Player::spawn(DOOMFIST);
    let (open, _) = open_and_walled(&doomfist.scene);
    let hold = |doomfist: &mut Player, buttons| {
        doomfist.step(Command {
            yaw: open,
            buttons,
            ..Command::default()
        });
    };
    // Game session s3 (D:/Temp/df5, `refs/reports/Doomfist.md` §12): a
    // 0.6 s charge (37 frames), the release at frame 11967 (here the first
    // frame without the button), the jump 9 frames later at 11976.
    for _ in 0..37 {
        hold(&mut doomfist, orpheus_wire::commands::SECONDARY);
    }
    for _ in 0..9 {
        hold(&mut doomfist, 0);
        assert!(doomfist.punch_state(42), "dashing");
    }
    hold(&mut doomfist, orpheus_wire::commands::JUMP);
    assert!(!doomfist.punch_state(42), "the jump ends the dash");
    // 11976: the jump's own frame still dashes at 45 m/s, 3.2 m/s up.
    let [vx, vy, vz] = doomfist.velocity();
    assert!((vx.hypot(vz) - 45.0).abs() < 0.01, "{vx} {vy} {vz}");
    assert!((vy - 3.2002).abs() < 0.001, "{vx} {vy} {vz}");
    // 11977: (-5.2217, 5.3994, -30.5439) m/s, 30.987 m/s along the facing.
    hold(&mut doomfist, 0);
    let [vx, vy, vz] = doomfist.velocity();
    assert!((vx.hypot(vz) - 30.987).abs() < 0.002, "{vx} {vy} {vz}");
    assert!((vy - 5.3994).abs() < 0.001, "{vx} {vy} {vz}");
    assert!(off_heading(vx, vz, open).abs() < 0.01, "{vx} {vz}");
}

/// Seismic Slam in the air (`0B27`): while he is more than 3 m above the
/// ground (st8's ray finds none) st7 runs the selector st6
/// (`STU_4C96470A`), the slam's target preview. The press (`#59` = 2, st63
/// sets `#1321`) makes it search the ground along his view from his eye
/// and write the point (`#431`) and its validity (`#1627`); the transition
/// `#1627 && #1321` then begins the `MoveTo` st9, which flies him at the
/// point. Rising Uppercut lifts him first; the slam is pressed as soon as
/// it is allowed, looking 33° down at the open floor ahead. The point is
/// at the edge of a step, and the straight flight to it meets the edge:
/// the move stops there (`m_725EB364`'s 0.1 s without 0.5 m of progress,
/// farther than 0.5 m from the point) and aborts (st54), as both his
/// client's flight and the server's did at a step edge in game session s4
/// (`refs/reports/Doomfist.md` §12: the server's blocked 0.68 m from the
/// point at frame 8851, his client's own flight touched the same edge at
/// 8852; st54 began on both).
#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn doomfists_airborne_slam_flies_at_the_ground_ahead_and_stops_at_a_step_edge() {
    let mut doomfist = Player::spawn(DOOMFIST);
    let (open, _) = open_and_walled(&doomfist.scene);
    let start = doomfist.position();
    let down = Command {
        yaw: open,
        pitch: 6000,
        ..Command::default()
    };
    doomfist.press(ABILITY_1, open);
    let mut previewing = None;
    for frame in 0..60 {
        doomfist.step(down);
        if doomfist.graph_state(0x0b27, 6) {
            previewing = Some((frame, doomfist.position()));
            break;
        }
    }
    let (shown, above) = previewing.expect("the airborne slam's preview began");
    assert!(above[1] > start[1] + 2.5, "preview at {shown}, {above:?}");
    let mut flying = None;
    let mut aborted = false;
    for frame in 1..=90 {
        let buttons = if flying.is_none() && frame % 2 == 1 {
            ABILITY_2
        } else {
            0
        };
        doomfist.step(Command { buttons, ..down });
        if flying.is_none() && doomfist.graph_state(0x0b27, 9) {
            flying = Some(frame);
        }
        aborted |= doomfist.graph_state(0x0b27, 54);
    }
    let Some(ScriptValue::Vector(point)) = doomfist.graph_variable(0x0b27, 431).cloned() else {
        panic!(
            "no landing point: {:?} #1627 {:?} #1321 {:?} {:?}",
            doomfist.graph_variable(0x0b27, 431),
            doomfist.graph_variable(0x0b27, 1627),
            doomfist.graph_variable(0x0b27, 1321),
            doomfist
                .snapshot()
                .assumptions
                .iter()
                .filter(|assumption| format!("{assumption:?}").contains("4C96470A"))
                .collect::<Vec<_>>()
        );
    };
    // The first press (frame 1) opens the search's gate, and its point
    // begins the flight in that frame (the game's frames:
    // `doomfists_tapped_airborne_slam_flies_from_the_press_frame`).
    assert_eq!(flying, Some(1), "the flight begins in the press's frame");
    // The point is ground below where he pressed and ahead of the spawn.
    assert!(point[1] < above[1] - 1.0, "{above:?} -> {point:?}");
    assert!(horizontal(start, point) > 3.0, "{start:?} -> {point:?}");
    assert!(aborted, "the flight stops at the step's edge");
}

/// The airborne slam's flight (`0B27` st9's `MoveTo`: 25 m/s, acceleration
/// 100, lateral time 0, 0.5 m in 0.1 s or stuck, exit limit 0 held at the
/// end) as Doomfist's client flew it in game session s4
/// (`refs/reports/Doomfist.md` §12; the client's movement state after each
/// command tick, mover `+0x10B0`): from his feet at frame 8830 toward the
/// point his landing search picked, 0.5269 m a move along the line, until
/// the capsule met the edge of the step beside the point (8852, its 21st
/// move: his states labelled 8840 and 8842 are one move apart) and came to
/// rest in the corner at 8853, 0.68 m from the point (the server's flight
/// stopped at the same spot to the millimetre).
#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn doomfists_airborne_slam_flight_stops_at_the_step_edge_where_his_client_did() {
    use orpheus_physics::mover::{Behaviour, MoveEnd, MoveKey, MoveTo, Speed};
    let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
    let body = scene.heroes().get(DOOMFIST).unwrap().body;
    let rays = scene.movement_index();
    let mut mover = scene
        .player_mover_at(body, 0, [55.400_390_625, 6.972_656_25, 38.931_640_625], 0)
        .unwrap();
    let flight = Behaviour::MoveTo(MoveTo {
        destination: [
            58.414_775_848_388_67,
            1.944_159_269_332_885_7,
            31.884_201_049_804_688,
        ],
        surface: [0.0; 3],
        free_spot: false,
        speed: Speed::MetresPerSecond(25.0),
        acceleration: 100.0,
        slow_radius: 0.0,
        lateral_seconds: 0.0,
        stuck_distance: 0.5,
        stuck_seconds: f64::from(0.1_f32),
        exit_horizontal: 10_000.0,
        exit_vertical: 10_000.0,
        exit_total: 0.0,
        exit_on_end: true,
        abort_past: false,
    });
    let key = MoveKey {
        instance: 1,
        raw_state: 9,
        activation: 1,
    };
    // The client's feet after his first, 8th, 15th, 20th and 21st move
    // (frames 8831, 8838, 8846, 8851 and 8852).
    let client: [(usize, [f64; 3]); 5] = [
        (0, [55.532_226_562_5, 6.752_929_687_5, 38.624_023_437_5]),
        (7, [56.455_078_125, 5.214_843_75, 36.470_703_125]),
        (14, [57.375_976_562_5, 3.677_734_375, 34.317_382_812_5]),
        (19, [58.033_203_125, 2.582_031_25, 32.779_296_875]),
        (20, [58.037_109_375, 2.461_914_062_5, 32.481_445_312_5]),
    ];
    // Where it came to rest (8853 on).
    let corner = [57.996_093_75, 2.364_257_812_5, 32.235_351_562_5];
    let mut feet = Vec::new();
    let mut end = None;
    for frame in 1..=40 {
        let command = Command {
            frame,
            ..Command::default()
        };
        end = mover
            .step_pose_to(rays, command, DT, Some((key, &flight)))
            .unwrap();
        feet.push(mover.pose().position);
        if end.is_some() {
            break;
        }
    }
    for (index, at) in client {
        let distance = (0..3)
            .map(|axis| (feet[index][axis] - at[axis]).powi(2))
            .sum::<f64>()
            .sqrt();
        assert!(
            distance < 0.01,
            "frame {index}: {:?} vs {at:?}",
            feet[index]
        );
    }
    assert_eq!(end, Some(MoveEnd::Blocked), "{feet:?}");
    let rest = *feet.last().unwrap();
    assert!(
        (0..3).all(|axis| (rest[axis] - corner[axis]).abs() < 0.01),
        "{rest:?} vs {corner:?}"
    );
}

/// Doomfist's slam cooldown `#61` (`0B27` st0's `m_out_CooldownVar`) as his
/// client keeps it (the Ability's update `0x7FF789B95560`): unlinked at 0
/// until the slam's end writes the cooldown (`rid70`, `#61 = #472`, 6 s),
/// linked from that write's frame (restart `0x7FF789BBB110`: the write's
/// event 2 runs the update's cooldown check at once) and unlinked at 0 again by the update its
/// timer runs 6 s later; then he slams again. Kept linked at its own 6.0,
/// his client used each ability once (df10 s1).
#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn doomfists_slam_cooldown_links_for_its_fall_and_he_slams_again() {
    let mut doomfist = Player::spawn(DOOMFIST);
    let (open, _) = open_and_walled(&doomfist.scene);
    let id = doomfist.instance_of(0x0b27);
    let cooldown = |player: &Player| {
        let snapshot = player.snapshot();
        let instance = &snapshot.instances[&id];
        (
            instance.variables.get(&61).cloned(),
            snapshot.bindings.contains_key(&(id, 61)),
            instance.states[&0]
                .ability_cooldown
                .map(|cooldown| cooldown.last_ms),
            snapshot.now_ms,
        )
    };
    let (value, bound, ..) = cooldown(&doomfist);
    assert_eq!((value, bound), (Some(ScriptValue::Float(0.0)), false));
    doomfist.press(ABILITY_2, open);
    let mut frames = Vec::new();
    for _ in 0..500 {
        doomfist.idle(1, open);
        frames.push(cooldown(&doomfist));
    }
    let first = frames.iter().position(|frame| frame.1).expect("linked");
    let last = first + frames[first..].iter().position(|frame| !frame.1).unwrap() - 1;
    let restart = frames[first].2.unwrap();
    // The write's frame: the restart happened in it.
    assert!(frames[first - 1].3 < restart && restart <= frames[first].3);
    for frame in &frames[first..=last] {
        assert_eq!(frame.0, Some(ScriptValue::Float(6.0)));
        assert_eq!(frame.2, Some(restart));
    }
    // Unlinked at 0 in the frame its 6 s end in.
    let end = restart + 6000;
    assert!(frames[last].3 < end && end <= frames[last + 1].3);
    assert!(
        frames[last + 1..]
            .iter()
            .all(|frame| (frame.0.clone(), frame.1) == (Some(ScriptValue::Float(0.0)), false))
    );
    doomfist.press(ABILITY_2, open);
    let [vx, vy, vz] = doomfist.velocity();
    assert!(
        (vx.hypot(vz).hypot(vy) - 19.0).abs() < 0.01,
        "{vx} {vy} {vz}"
    );
}

/// Rocket Punch held through Rising Uppercut, and Rising Uppercut pressed
/// in the punch's last frame, start when the other stops (the user's check
/// of 2026-11-21, issues 3 and 5; `refs/reports/Doomfist.md` §20). Each of
/// his abilities pushes itself on #33e at one priority (`0C4…0001`), and
/// his client's start check (0x7FF789BB8D20) asks the stack for that
/// priority before it takes a press, puts the press back when the stack
/// refuses, and runs again when the stack changes. The server took the
/// press, pushed the ability under the running one, which dropped it, and
/// latched the button (flag 4), which its frames gave his client too: the
/// punch never started while secondary fire was held, and the uppercut
/// his client began at the punch's end was rolled back. The inputs are
/// game session `before`'s (Practice Range, `D:/Temp/dfc`), frames counted
/// from the first press: there the uppercut ran 11481-11517 and the punch
/// 15584-15639 on both sides.
#[test]
#[ignore = "requires the derived 104319 data and the Practice Range collision"]
fn doomfists_ability_pressed_while_another_runs_starts_when_it_stops() {
    use orpheus_wire::commands::SECONDARY;
    // Rising Uppercut's subgraph state (`0DD8` st6) and Rocket Punch's
    // (`0D06` st3) each frame, and the frames where they change.
    let run = |buttons: fn(u32) -> u32, frames: u32| {
        let mut doomfist = Player::spawn(DOOMFIST);
        let (open, _) = open_and_walled(&doomfist.scene);
        let mut changes = Vec::new();
        let mut last = (false, false);
        for frame in 0..frames {
            doomfist.step(Command {
                yaw: open,
                buttons: buttons(frame),
                ..Command::default()
            });
            let now = (doomfist.graph_state(0x0dd8, 6), doomfist.punch_state(3));
            if now != last {
                changes.push((frame, now));
                last = now;
            }
        }
        changes
    };
    // Issue 3 (session `up_hold200`): Shift for 3 frames, secondary fire
    // held from frame 15 (inside the uppercut) to 103. The uppercut stops
    // at 37 (st21, 0.35 s after its rise) and the punch begins that frame,
    // as his client's did (`after`, the frame its own uppercut stopped).
    let uppercut_then_punch = run(
        |frame| match frame {
            0..=2 => ABILITY_1,
            15..=103 => SECONDARY,
            _ => 0,
        },
        110,
    );
    assert_eq!(
        uppercut_then_punch,
        [(0, (true, false)), (37, (false, true))]
    );
    // Issue 5 (session `punch_up528`): secondary fire for 20 frames, Shift
    // from 55, one frame before the punch stops (its ability ended at 56 on
    // both sides); his client began the uppercut at 56.
    let punch_then_uppercut = run(
        |frame| match frame {
            0..=19 => SECONDARY,
            55..=58 => ABILITY_1,
            _ => 0,
        },
        70,
    );
    assert_eq!(
        punch_then_uppercut,
        [(0, (false, true)), (56, (true, false))]
    );
}
