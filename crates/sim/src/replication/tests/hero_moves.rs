//! Heroes' moves through the player's tick: the body script on the derived
//! Practice Range scene, with its collision. Tracer's Blink and Recall;
//! Pharah's Jump Jet; Echo's ground ray. Tracer's graphs still reach a config class the VM
//! doesn't compute (`STU_038C32E8`) and `StateBlinkEffect`, neither of
//! which decides the moves tested here, so the body tolerates unknowns as
//! release builds do.

use super::*;
use orpheus_script::{vm::Unknowns, wire::Value as ScriptValue};
use orpheus_wire::commands::Command;

const TRACER: u64 = 0x02e0_0000_0000_0003;
const PHARAH: u64 = 0x02e0_0000_0000_0008;
const ECHO: u64 = 0x02e0_0000_0000_0206;
/// Shift and E on the command (logical buttons 35 and 36).
const BLINK: u32 = 0x08;
const RECALL: u32 = 0x10;
const DT: f64 = FRAME_MS as f64 / 1000.0;

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
    use orpheus_physics::mover::{MoveKey, MoveTo, Speed};
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
            .step_pose_to(scene.movement_index(), command, DT, Some((key, &pull)))
            .unwrap();
        let target = mover.move_target().unwrap();
        // Within a few f32 steps of the recorded values.
        assert!(
            (0..3).all(|axis| (target[axis] - f64::from(client[axis])).abs() < 2e-5),
            "{target:?} vs the client's {client:?}"
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
    use orpheus_physics::mover::{MoveEnd, MoveKey, MoveTo, Speed};
    const WIDOWMAKER: u64 = 0x02e0_0000_0000_000a;
    let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
    let body = scene.heroes().get(WIDOWMAKER).unwrap().body;
    let rays = scene.movement_index();
    let mut mover = scene
        .player_mover_at(body, 0, [54.565_43, 1.0, 42.147_46], 0)
        .unwrap();
    // The kind-1 block her client began (destination `+0x30`, surface
    // `+0x40`) and `005A` st1's numbers.
    let pull = MoveTo {
        destination: [60.397_644, 4.622_823, 42.147_46],
        surface: [-0.985_831, 0.0, 0.167_742],
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
    let command = |frame| Command {
        frame,
        ..Command::default()
    };
    let mut frame = 0;
    let end = loop {
        frame += 1;
        let outcome = mover
            .step_pose_to(rays, command(frame), DT, Some((key, &pull)))
            .unwrap();
        if let Some(end) = outcome {
            break end;
        }
    };
    // Her client's behaviour drove 19 ticks; the velocity its end (slot 5
    // `0x7FF789B539B0`) found, f32 as recorded.
    assert_eq!((frame, end), (19, MoveEnd::Blocked));
    assert_eq!(
        mover.pose().velocity,
        [0.554_687_5, 8.826_171_875, 3.257_812_5]
    );
    let mut path = Vec::new();
    for _ in 0..120 {
        frame += 1;
        mover.step_pose_to(rays, command(frame), DT, None).unwrap();
        path.push(mover.pose().position);
    }
    // Two of her client's samples (component 4, the feet plus 1 m, as
    // printed): past the wall's top lip, and past the next wall.
    for sample in [[59.4990, 6.5518, 43.9248], [59.1377, 5.9805, 44.6191]] {
        let feet = [sample[0], sample[1] - 1.0, sample[2]];
        assert!(
            path.iter()
                .any(|at| (0..3).all(|axis| (at[axis] - feet[axis]).abs() < 1e-4)),
            "{feet:?} not on {path:?}"
        );
    }
    // Where her client rested (component 4 (58.128906, 2.0, 44.796875)).
    assert_eq!(mover.pose().position, [58.128_906_25, 1.0, 44.796_875]);
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
