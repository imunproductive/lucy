use super::*;
use serde_json::json;

const BODY: u64 = 0x0400_0000_0000_1403;
const ALIAS: &str = "0x0C80000000000451";

/// A clip of `seconds` at 10 fps keying the body bone's height.
fn heights(seconds: f64, keys: &[(u32, f64)], additive: bool) -> Json {
    json!({
        "additive": additive,
        "duration": seconds,
        "fps": 10.0,
        "frames": (seconds * 10.0) as usize + 1,
        "tracks": [{
            "bone": 1,
            "positions": keys.iter().map(|(frame, y)| json!([frame, 0.0, y, 0.0])).collect::<Vec<_>>(),
        }],
    })
}

fn branch(base: &[(&str, bool)], layers: &[&str]) -> Json {
    json!({
        "base": base.iter().map(|(clip, once)| json!({"clip": clip, "once": once})).collect::<Vec<_>>(),
        "layers": layers,
    })
}

/// A root, a body bone 1 m up holding a sphere at its origin, and clips:
/// standing `A` (the body 1 m up, every frame) with the bob `BOB` (+0.1 m
/// at half its 4 s); moving `B` once (2 m, 1 s) then `C` (3 m, looping);
/// the anim `D` (from 4 to 5 m over 2 s).
fn rig() -> BotRigCatalog {
    let identity = json!({"rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0]});
    let bone = |parent: Option<usize>, y: f64| {
        let mut bone = identity.clone();
        bone["parent"] = json!(parent);
        bone["translation"] = json!([0.0, y, 0.0]);
        bone["id"] = json!(parent.map_or(1, |_| 2));
        bone
    };
    let file = json!({
        "bodies": {"0x0400000000001403": {"name": "Training Bot", "rig": "rig", "shapes": [
            {"bone": 1, "kind": "sphere", "centre": [0.0, 0.0, 0.0], "radius": 0.4, "critical": true},
        ]}},
        "rigs": {"rig": {
            "bones": [bone(None, 0.0), bone(Some(0), 0.5)],
            "branches": {
                "standing": branch(&[("A", false)], &["BOB"]),
                "moving": branch(&[("B", true), ("C", false)], &[]),
                "arms_forward_standing": branch(&[("C", false)], &[]),
                "arms_forward_moving": branch(&[("C", false)], &[]),
            },
            "anims": {ALIAS: {
                "plain": branch(&[("D", false)], &[]),
                "arms_forward": branch(&[("D", false)], &[]),
            }},
            "clips": {
                "A": heights(0.3, &[(0, 1.0)], false),
                "BOB": heights(4.0, &[(0, 0.0), (20, 0.1), (40, 0.0)], true),
                "B": heights(1.0, &[(0, 2.0)], false),
                "C": heights(1.0, &[(0, 3.0), (10, 3.5)], false),
                "D": heights(2.0, &[(0, 4.0), (20, 5.0)], false),
            },
        }},
    });
    BotRigCatalog::from_json(&file.to_string()).unwrap()
}

fn height(input: PoseInput) -> f64 {
    rig().for_body(BODY).unwrap().shapes(&input)[0].a[1]
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn a_looping_clip_wraps_and_a_held_one_stops_at_its_end() {
    let catalog = rig();
    let rig = &catalog.for_body(BODY).unwrap().rig;
    let c = rig
        .clips
        .iter()
        .find(|clip| clip.tracks[0].positions.last().unwrap().1[1] == 3.5)
        .unwrap();
    assert!(close(c.frame(0.25, true), 2.5));
    assert!(close(c.frame(1.25, true), 2.5));
    assert!(close(c.frame(-0.75, true), 2.5));
    assert!(close(c.frame(1.25, false), 10.0));
    // Sampled between its keys: 3 m to 3.5 m over the second.
    let mut pose = rig.rest.clone();
    c.replace(c.frame(1.5, true), &mut pose);
    assert!(close(pose[1].translation[1], 3.25), "{pose:?}");
}

#[test]
fn a_layer_adds_its_translation_and_turns_after_the_base() {
    let quarter = std::f64::consts::FRAC_1_SQRT_2;
    let layer = Clip {
        duration: 1.0,
        fps: 10.0,
        last: 10.0,
        tracks: vec![Track {
            bone: 0,
            positions: vec![(0.0, [0.0, 0.1, 0.0])],
            rotations: vec![(0.0, [0.0, quarter, 0.0, quarter])],
            scales: Vec::new(),
        }],
    };
    let base = Transform {
        // A quarter turn about +X.
        rotation: [quarter, 0.0, 0.0, quarter],
        scale: [1.0; 3],
        translation: [0.0, 1.0, 0.0],
    };
    let mut pose = [base];
    layer.add(0.0, &mut pose);
    assert_eq!(pose[0].translation, [0.0, 1.1, 0.0]);
    assert_eq!(pose[0].scale, [1.0; 3]);
    // base · delta: +Y stays on +Y under the delta's turn about +Y, then
    // goes to +Z under the base's turn about +X (delta · base would take it
    // to +X).
    let up = rotate(pose[0].rotation, [0.0, 1.0, 0.0]);
    let expected = rotate(
        base.rotation,
        rotate([0.0, quarter, 0.0, quarter], [0.0, 1.0, 0.0]),
    );
    assert!((0..3).all(|k| close(up[k], expected[k])), "{up:?}");
    assert!(close(up[2], 1.0), "{up:?}");
}

#[test]
fn a_walking_bot_plays_the_moving_branch_from_where_it_set_off() {
    // Standing: the body bone 1 m up (clip A), plus the bob at 0 s.
    assert!(close(height(PoseInput::default()), 1.0));
    // Moving: B for its first second, then C looping.
    let moving = |seconds| PoseInput {
        moving: Some(seconds),
        age: 100.0,
        ..PoseInput::default()
    };
    assert!(close(height(moving(0.5)), 2.0));
    assert!(close(height(moving(1.5)), 3.25));
    assert!(close(height(moving(2.5)), 3.25));
    // The arms-forward category picks its own branch.
    assert!(close(
        height(PoseInput {
            arms_forward: true,
            ..PoseInput::default()
        }),
        3.0
    ));

    let mut animation = BotAnimation::new(rig().for_body(BODY).unwrap());
    animation.advance(100, false, false, None);
    assert!(close(animation.shapes()[0].a[1], 1.0));
    animation.advance(110, true, false, None);
    assert!(close(animation.shapes()[0].a[1], 2.0));
    // 1.2 s after it set off: C at 0.2 s.
    animation.advance(185, true, false, None);
    assert!(
        close(animation.shapes()[0].a[1], 3.1),
        "{:?}",
        animation.shapes()
    );
    animation.advance(186, false, false, None);
    let standing = animation.shapes()[0].a[1];
    assert!((1.0..1.1).contains(&standing), "{standing}");
}

#[test]
fn the_posed_shapes_follow_the_bob_over_time() {
    let at = |age| {
        height(PoseInput {
            age,
            ..PoseInput::default()
        })
    };
    assert!(close(at(0.0), 1.0));
    assert!(close(at(1.0), 1.05));
    assert!(close(at(2.0), 1.1));
    assert!(close(at(3.0), 1.05));
    // The bob loops every 4 s.
    assert!(close(at(6.0), 1.1));
    let animation = BotAnimation::new(rig().for_body(BODY).unwrap());
    let mut later = animation.fresh();
    later.advance(1000, false, false, None);
    later.advance(1125, false, false, None);
    assert!(close(later.shapes()[0].a[1], 1.1));
    // A respawn's clock starts over.
    let mut fresh = later.fresh();
    fresh.advance(2000, false, false, None);
    assert!(close(fresh.shapes()[0].a[1], 1.0));
}

#[test]
fn a_script_anim_replaces_the_base_and_holds_at_its_end() {
    let alias = parse_guid(ALIAS).unwrap();
    let anim = |elapsed, over| PoseInput {
        anim: Some(AnimRequest {
            alias,
            elapsed,
            over,
        }),
        ..PoseInput::default()
    };
    assert!(close(height(anim(1.0, None)), 4.5));
    // Played over 4 s, the 2 s clip runs at half speed.
    assert!(close(height(anim(1.0, Some(4.0))), 4.25));
    assert!(close(height(anim(9.0, None)), 5.0));
    // An alias the rig doesn't bind leaves the locomotion.
    let unknown = PoseInput {
        anim: Some(AnimRequest {
            alias: 4,
            elapsed: 1.0,
            over: None,
        }),
        ..PoseInput::default()
    };
    assert!(close(height(unknown), 1.0));
}

#[test]
fn a_rig_refuses_a_bone_before_its_parent_and_an_unknown_clip() {
    let bones = json!([
        {"id": 1, "parent": 1, "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0], "translation": [0.0, 0.0, 0.0]},
        {"id": 2, "parent": null, "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0], "translation": [0.0, 0.0, 0.0]},
    ]);
    let empty = branch(&[], &[]);
    let file = |bones: &Json, standing: Json| {
        json!({"bodies": {}, "rigs": {"rig": {
            "bones": bones,
            "branches": {"standing": standing, "moving": empty, "arms_forward_standing": empty, "arms_forward_moving": empty},
            "anims": {},
            "clips": {},
        }}})
        .to_string()
    };
    assert!(BotRigCatalog::from_json(&file(&bones, empty.clone())).is_err());
    let ordered = json!([bones[1]]);
    assert!(BotRigCatalog::from_json(&file(&ordered, empty.clone())).is_ok());
    assert!(BotRigCatalog::from_json(&file(&ordered, branch(&[("X", false)], &[]))).is_err());
}

mod game {
    use crate::{scene_assets::PreparedScene, scene_assets::SceneAssets};
    use orpheus_content::maps::GameMap;
    use orpheus_script::vm::Unknowns;

    const TRAINING_BOT: u64 = 0x0400_0000_0000_1403;

    fn practice_range() -> PreparedScene {
        SceneAssets::load(orpheus_testkit::runtime_dir())
            .unwrap()
            .prepare(GameMap::practice_range())
            .unwrap()
    }

    /// The Practice Range with its bots' scripts, which ask for the
    /// patrols' walks (they hold content the VM doesn't know yet).
    fn scripted_practice_range() -> PreparedScene {
        let mut assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
        assets.set_bot_unknowns(Unknowns::Tolerate);
        assets.prepare(GameMap::practice_range()).unwrap()
    }

    /// The bots' tick as the match runs it: they walk, then their scripts
    /// run what is due (the patrol's waits).
    fn step(scene: &mut PreparedScene, tick: u32) {
        scene.step_bots(tick);
        scene.advance_live_bots(tick);
    }

    /// The critical shape's centre height over the bot's feet.
    fn head(scene: &PreparedScene, slot: usize) -> f64 {
        let target = scene.bot_target(&scene.bots()[slot]);
        let shapes = target.shapes.unwrap();
        shapes.iter().find(|shape| shape.critical).unwrap().a[1]
    }

    /// A standing Training Bot hovers by its bob layer `391F` over the 4 s
    /// loop: its head sphere's centre from 1.930 to 2.007 m, 1.972 on
    /// average (the idle pose's 1.979 within).
    #[test]
    #[ignore = "requires a derived 104319 Practice Range scene"]
    fn the_training_bots_head_bobs_over_its_loop() {
        let mut scene = practice_range();
        let slot = scene
            .bots()
            .iter()
            .position(|bot| bot.spawn.body == TRAINING_BOT && bot.spawn.route.is_empty())
            .unwrap();
        let mut heights = Vec::new();
        // 4 s of 16 ms ticks.
        for tick in 1000..1250 {
            scene.step_bots(tick);
            heights.push(head(&scene, slot));
        }
        let low = heights.iter().copied().fold(f64::INFINITY, f64::min);
        let high = heights.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let mean = heights.iter().sum::<f64>() / heights.len() as f64;
        assert!((low - 1.930).abs() < 0.002, "{low} {high} {mean}");
        assert!((high - 2.007).abs() < 0.002, "{low} {high} {mean}");
        assert!((mean - 1.972).abs() < 0.002, "{low} {high} {mean}");
        // One loop later the bob is where it was.
        scene.step_bots(1250);
        assert!((head(&scene, slot) - heights[0]).abs() < 1e-3);
    }

    /// A Practice Range bot walking its route is posed by the moving
    /// branch (`3990`, then `3952`): its shapes are not the standing ones.
    #[test]
    #[ignore = "requires a derived 104319 Practice Range scene"]
    fn a_walking_bot_is_posed_apart_from_standing() {
        let mut scene = scripted_practice_range();
        let slot = scene
            .bots()
            .iter()
            .position(|bot| !bot.spawn.route.is_empty())
            .unwrap();
        let mut tick = 1000;
        step(&mut scene, tick);
        let standing = scene.bot_target(&scene.bots()[slot]).shapes.unwrap();
        while !scene.bots()[slot].walking() {
            tick += 1;
            step(&mut scene, tick);
        }
        // Two seconds into the walk: past the setting-off clip.
        for _ in 0..125 {
            tick += 1;
            step(&mut scene, tick);
        }
        assert!(scene.bots()[slot].walking());
        let walking = scene.bot_target(&scene.bots()[slot]).shapes.unwrap();
        assert_eq!(walking.len(), standing.len());
        let moved = walking
            .iter()
            .zip(standing.iter())
            .map(|(walking, standing)| {
                (0..3)
                    .map(|k| (walking.a[k] - standing.a[k]).abs())
                    .fold(0.0, f64::max)
            })
            .fold(0.0, f64::max);
        assert!(moved > 0.02, "{walking:?} {standing:?}");
    }
}

/// A Training Bot's assembly (`0BA8` st1, alias `0451` over its 2.5 s,
/// clip `39D7` of 2.667 s) poses its bones where the client draws them.
/// Measured in the game (2026-11-21, owprobe, the client's own hit-test
/// skeleton, `*(*(model + 0x6F0) + 0x28)` bones 1 and 34, read every
/// 10 ms after the respawned bot appeared; refs report `CombatBugs.md`
/// §4): the body bone holds 1.62 m up with the head at 1.69 while the
/// parts settle, the head then flies up past 4 m, drops through 0.8 m and
/// swings to 0.5 m behind at 1.14 to 1.39 m, and settles at 1.80 m; the
/// server's pose follows the same course, the client's about 0.1 s later
/// (its view delay). So the head is hit where it is drawn. While the body
/// bone holds at 1.62 m the head bone is scaled down: the client's read
/// 0.034 (2026-11-22, refs report `CombatFeel.md`), its own track's
/// scale, not that times its parent's, so the head's shape is 0.44 ×
/// 0.034 m round then, as in the client's hit test.
#[test]
#[ignore = "requires a derived 104319 Practice Range scene"]
fn the_assembly_poses_the_bones_where_the_client_draws_them() {
    let path = orpheus_testkit::runtime_dir().join("data/game_bot_animation_174.json");
    let catalog = BotRigCatalog::load(path).unwrap();
    let body = catalog.for_body(0x0400_0000_0000_1403).unwrap();
    let bones = |t: f64| {
        let places = body.rig.places(&PoseInput {
            age: t,
            anim: Some(AnimRequest {
                alias: 0x0C80_0000_0000_0451,
                elapsed: t,
                over: Some(2.5),
            }),
            ..PoseInput::default()
        });
        (places[1].translation, places[34].translation)
    };
    // (seconds into the server's pose, the client's body and head bones
    // 0.1 s later).
    for (t, client_body, client_head) in [
        (0.3, [0.00, 1.62, 0.00], [-0.01, 1.70, 0.03]),
        (1.3, [-0.05, 1.33, -0.01], [-0.10, 1.25, -0.51]),
        (2.5, [-0.00, 1.30, 0.00], [-0.00, 1.80, -0.03]),
    ] {
        let (body, head) = bones(t);
        for k in 0..3 {
            assert!(
                (body[k] - client_body[k]).abs() < 0.06,
                "{t}: body {body:?}"
            );
            assert!(
                (head[k] - client_head[k]).abs() < 0.12,
                "{t}: head {head:?}"
            );
        }
    }
    let input = PoseInput {
        age: 0.3,
        anim: Some(AnimRequest {
            alias: 0x0C80_0000_0000_0451,
            elapsed: 0.3,
            over: Some(2.5),
        }),
        ..PoseInput::default()
    };
    let scale = body.rig.places(&input)[34].scale;
    assert!(
        scale.iter().all(|s| (s - 0.034).abs() < 0.0005),
        "{scale:?}"
    );
    let head = body.shapes(&input)[0];
    assert!(head.critical);
    assert!((head.radius - 0.44 * scale[0]).abs() < 1e-9, "{head:?}");
}
