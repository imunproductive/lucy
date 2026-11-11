use serde_json::{Value, json};

use super::*;

const FLOOR: f32 = 0.35;
const SPEED: f64 = 5.5;

/// A volume of the given box with the listed cells walkable (the layout of
/// `data/navigation`, see `orpheus_physics::nav`).
fn volume(
    origin: [i32; 3],
    size: [i32; 3],
    cells: &[[i32; 3]],
    floor: [f32; 3],
    links: &[usize],
) -> Value {
    let count = (size[0] * size[1] * size[2]) as usize;
    let mut walkable = vec![0u32; count.div_ceil(32)];
    for cell in cells {
        let [x, y, z] = [0, 1, 2].map(|axis| cell[axis] - origin[axis]);
        let bit = (z + size[2] * (x + size[0] * y)) as usize;
        walkable[bit / 32] |= 1 << (bit % 32);
    }
    json!({"floor": floor, "origin": origin, "size": size, "neighbours": links, "walkable": walkable})
}

fn tile(x0: i32, z0: i32) -> Vec<[i32; 3]> {
    (x0..x0 + 8)
        .flat_map(|x| (z0..z0 + 8).map(move |z| [x, 0, z]))
        .collect()
}

/// Four 8 × 8 tiles in a ring; 0 and 3 touch but aren't linked (a wall),
/// so from 0 to 3 the way goes round through 1 and 2:
///
/// ```text
/// z 16 +---+---+
///      | 3 | 2 |
///    8 +---+---+
///      | 0 | 1 |
///    0 +---+---+
///      0   8   16 x
/// ```
fn ring() -> Navigation {
    let file = json!({"volumes": [
        volume([0, 0, 0], [8, 1, 8], &tile(0, 0), [3.5, FLOOR, 3.5], &[1]),
        volume([8, 0, 0], [8, 1, 8], &tile(8, 0), [11.5, FLOOR, 3.5], &[0, 2]),
        volume([8, 0, 8], [8, 1, 8], &tile(8, 8), [11.5, FLOOR, 11.5], &[1, 3]),
        volume([0, 0, 8], [8, 1, 8], &tile(0, 8), [3.5, FLOOR, 11.5], &[2]),
    ]});
    Navigation::from_json(&serde_json::to_vec(&file).unwrap()).unwrap()
}

fn clear(_: [f64; 3], _: [f64; 3]) -> bool {
    true
}

fn blind(_: [f64; 3], _: [f64; 3]) -> bool {
    false
}

fn enemy(slot: usize, feet: [f64; 3]) -> Seen {
    Seen {
        slot,
        team: 1,
        feet,
        height: 1.8,
    }
}

fn senses<'a>(
    tick: u32,
    feet: [f64; 3],
    others: &'a [Seen],
    goal: Goal,
    navigation: Option<&'a Navigation>,
    sight: &'a dyn Fn([f64; 3], [f64; 3]) -> bool,
) -> Senses<'a> {
    Senses {
        tick,
        team: 0,
        feet: Some(feet),
        body: Some(0xa000_0101),
        eye: 1.5,
        health: 1.0,
        others,
        goal,
        home: [1.0, 0.0, 1.0],
        navigation,
        sight,
    }
}

/// Where a body goes in a tick with this command, on flat ground: the
/// throttles turned by the yaw (`orpheus_wire::commands`), at a run.
fn walk(feet: [f64; 3], command: &Command) -> [f64; 3] {
    let yaw = f64::from(command.yaw) * TAU / 65536.0;
    let (sin, cos) = yaw.sin_cos();
    let (forward, right) = (
        f64::from(command.forward) / 127.0,
        f64::from(command.right) / 127.0,
    );
    let dx = forward * sin - right * cos;
    let dz = forward * cos + right * sin;
    let length = dx.hypot(dz);
    if length == 0.0 {
        return feet;
    }
    let step = SPEED * TICK_PERIOD.as_secs_f64() / length.max(1.0);
    [feet[0] + dx * step, feet[1], feet[2] + dz * step]
}

#[test]
fn difficulty_scales_damage_and_accuracy_as_the_rules_say() {
    let levels: Vec<_> = (1..=3)
        .map(|level| Difficulty::from_level(level).unwrap())
        .collect();
    assert_eq!(
        levels,
        [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard]
    );
    assert_eq!(Difficulty::from_level(0), None);
    assert_eq!(Difficulty::from_level(4), None);
    // Rule 123: 25%, 50% and 100% of the damage.
    let scales: Vec<_> = levels.iter().map(|level| level.damage_scale()).collect();
    assert_eq!(scales, [0.25, 0.5, 1.0]);
    // Harder bots aim better and react sooner.
    assert!(
        levels
            .windows(2)
            .all(|pair| pair[0].aim_error() > pair[1].aim_error())
    );
    assert!(
        levels
            .windows(2)
            .all(|pair| pair[0].reaction() > pair[1].reaction())
    );
    assert_eq!(Difficulty::parse(" Hard "), Some(Difficulty::Hard));
    assert_eq!(Difficulty::parse("2"), Some(Difficulty::Medium));
    assert_eq!(Difficulty::parse("insane"), None);
}

/// The spread of the aims of an AI that sees an enemy 20 m ahead is
/// wider the easier it is, and every aim stays within its error.
#[test]
fn easier_ai_misses_its_aim_by_more() {
    let others = [enemy(4, [0.0, 0.0, 20.0])];
    let spread = |difficulty: Difficulty| {
        let mut ai = AiController::new(difficulty, 0, 1, 7);
        let (yaw, pitch) = aim_at([0.0, 1.5, 0.0], others[0].chest());
        let most = (difficulty.aim_error() * 65536.0 / TAU).ceil() as i32;
        let mut total = 0.0;
        for tick in 0..400 {
            let command = ai.command(&senses(tick, [0.0; 3], &others, Goal::Hold, None, &clear));
            let (dy, dp) = (command.yaw - yaw, command.pitch - pitch);
            assert!(
                dy.abs() <= most && dp.abs() <= most,
                "{difficulty:?}: {dy} {dp}"
            );
            total += f64::from(dy.abs());
        }
        total / 400.0
    };
    let [easy, medium, hard] = [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard].map(spread);
    assert!(easy > medium && medium > hard, "{easy} {medium} {hard}");
}

#[test]
fn angles_follow_the_command_axes() {
    let eye = [0.0, 1.5, 0.0];
    // Forward is +z at yaw 0, +x at a quarter turn; the pitch is positive
    // looking down.
    assert_eq!(aim_at(eye, [0.0, 1.5, 10.0]), (0, 0));
    assert_eq!(aim_at(eye, [10.0, 1.5, 0.0]), (16384, 0));
    assert_eq!(aim_at(eye, [0.0, -8.5, 10.0]), (0, 8192));
    // Right is (-cos yaw, 0, sin yaw): looking +z, walking +x is a full
    // left (-127).
    assert_eq!(throttles(0, [1.0, 0.0]), (0, -127));
    assert_eq!(throttles(0, [0.0, 1.0]), (127, 0));
    assert_eq!(throttles(16384, [1.0, 0.0]), (127, 0));
    assert_eq!(throttles(0, [1.0, 1.0]), (127, -127));
    assert_eq!(throttles(0, [0.0, 0.0]), (0, 0));
}

/// The nearest enemy it can see within range; not a teammate, not one
/// behind a wall, not one out of range; the current target stays unless
/// another is much nearer.
#[test]
fn target_choice_takes_the_nearest_visible_enemy_and_keeps_it() {
    let eye = [0.0, 1.5, 0.0];
    let mut mate = enemy(1, [0.0, 0.0, 3.0]);
    mate.team = 0;
    let others = [
        mate,
        enemy(2, [0.0, 0.0, 12.0]),
        enemy(3, [0.0, 0.0, 8.0]),
        enemy(4, [5.0, 0.0, 0.0]),
        enemy(5, [0.0, 0.0, 60.0]),
    ];
    let walled = |other: &Seen| other.slot != 4;
    assert_eq!(
        choose_target(eye, 0, &others, None, ENGAGE_RANGE, walled),
        Some(3)
    );
    assert_eq!(
        choose_target(eye, 0, &others, None, ENGAGE_RANGE, |_| true),
        Some(4)
    );
    // Slot 2 is 12 m away, the nearest 8 m: kept.
    assert_eq!(
        choose_target(eye, 0, &others, Some(2), ENGAGE_RANGE, walled),
        Some(2)
    );
    // Slot 2 against one at 5 m: less than half, so it changes.
    assert_eq!(
        choose_target(eye, 0, &others, Some(2), ENGAGE_RANGE, |_| true),
        Some(4)
    );
    // Out of range or hidden: none.
    assert_eq!(
        choose_target(eye, 0, &others[4..], None, ENGAGE_RANGE, |_| true),
        None
    );
    assert_eq!(
        choose_target(eye, 0, &others, None, ENGAGE_RANGE, |_| false),
        None
    );
}

/// An AI shoots at an enemy it sees once its reaction time is over, in
/// bursts, at his body's centre; one behind a wall it doesn't shoot.
#[test]
fn an_ai_fires_at_a_visible_enemy_after_its_reaction_time() {
    let others = [enemy(4, [0.0, 0.0, 15.0])];
    let mut ai = AiController::new(Difficulty::Hard, 0, 1, 1);
    let reaction = ticks(Difficulty::Hard.reaction());
    let fired: Vec<bool> = (0..200)
        .map(|tick| {
            let command = ai.command(&senses(tick, [0.0; 3], &others, Goal::Hold, None, &clear));
            command.buttons & FIRE != 0
        })
        .collect();
    assert_eq!(ai.target(), Some(4));
    assert!(fired[..reaction as usize].iter().all(|&fire| !fire));
    assert!(fired[reaction as usize]);
    // Bursts: it stops and starts again.
    let shots = fired.iter().filter(|&&fire| fire).count();
    assert!(shots > 60 && shots < 200 - reaction as usize, "{shots}");
    let mut ai = AiController::new(Difficulty::Hard, 0, 1, 1);
    for tick in 0..200 {
        let command = ai.command(&senses(tick, [0.0; 3], &others, Goal::Hold, None, &blind));
        assert_eq!(command.buttons & FIRE, 0);
    }
    assert_eq!(ai.target(), None);
}

#[test]
fn steering_passes_reached_waypoints() {
    let waypoints = [[0.0, 0.0, 0.0], [5.0, 0.0, 0.0], [5.0, 0.0, 5.0]];
    let mut next = 0;
    assert_eq!(
        steer([0.1, 0.0, 0.0], &waypoints, &mut next),
        Some(waypoints[1])
    );
    assert_eq!(next, 1);
    assert_eq!(
        steer([4.5, 0.0, 0.2], &waypoints, &mut next),
        Some(waypoints[2])
    );
    assert_eq!(next, 2);
    assert_eq!(steer([5.0, 0.0, 4.0], &waypoints, &mut next), None);
}

/// From tile 0 to an objective on tile 3 the AI walks round the wall
/// between them (0 and 3 aren't linked): it crosses from z < 8 to z >= 8
/// only at the corner with tiles 1 and 2, and stops at the objective.
#[test]
fn an_ai_follows_the_navigation_path_to_the_objective() {
    let nav = ring();
    let objective = [3.5, 0.35, 12.5];
    let mut ai = AiController::new(Difficulty::Easy, 0, 1, 3);
    let mut feet = [2.5, 0.35, 2.5];
    let mut arrived = None;
    for tick in 0..2000 {
        let command = ai.command(&senses(
            tick,
            feet,
            &[],
            Goal::Objective(objective),
            Some(&nav),
            &clear,
        ));
        if command.forward == 0 && command.right == 0 && tick > 0 {
            arrived = Some(tick);
            break;
        }
        let step = walk(feet, &command);
        if feet[2] < 8.0 && step[2] >= 8.0 {
            assert!(step[0] > 6.5, "through the wall at {step:?}");
        }
        feet = step;
    }
    let arrived = arrived.expect("the AI arrives");
    assert!(flat_distance(feet, objective) <= HOLD, "{feet:?}");
    // Round the wall's end at x 8 (about 15 m) rather than straight up
    // (10 m).
    assert!(f64::from(arrived) * TICK_PERIOD.as_secs_f64() * SPEED > 11.0);
}

/// Each AI of a team takes its own cell around the objective, and leaves
/// the spawn 1.5 s after the one before it.
#[test]
fn a_team_spreads_over_the_objective_and_leaves_staggered() {
    let nav = ring();
    let objective = [11.5, 0.35, 11.5];
    let start = [2.5, 0.35, 2.5];
    let mut cells = Vec::new();
    let mut first_steps = Vec::new();
    for rank in 0..3 {
        let mut ai = AiController::new(Difficulty::Medium, rank, 3, 9);
        let mut feet = start;
        let mut first = None;
        for tick in 0..3000 {
            let command = ai.command(&senses(
                tick,
                feet,
                &[],
                Goal::Objective(objective),
                Some(&nav),
                &clear,
            ));
            let moving = command.forward != 0 || command.right != 0;
            if moving && first.is_none() {
                first = Some(tick);
            }
            if !moving && first.is_some() {
                break;
            }
            feet = walk(feet, &command);
        }
        first_steps.push(first.unwrap());
        cells.push(feet);
    }
    assert_eq!(first_steps, [0, ticks(1.5), ticks(3.0)]);
    for (index, a) in cells.iter().enumerate() {
        assert!(flat_distance(*a, objective) <= f64::from(SPREAD) + HOLD);
        for b in &cells[index + 1..] {
            assert!(flat_distance(*a, *b) > 1.5, "{cells:?}");
        }
    }
}

/// Hurt below 30%, it falls back to its spawn and uses its ability once;
/// healed again, it goes back to the objective.
#[test]
fn a_hurt_ai_falls_back_to_its_spawn() {
    let objective = [20.0, 0.0, 0.0];
    let mut ai = AiController::new(Difficulty::Medium, 0, 1, 5);
    let feet = [10.0, 0.0, 0.0];
    let mut hurt = senses(10, feet, &[], Goal::Objective(objective), None, &clear);
    hurt.health = 0.2;
    let command = ai.command(&hurt);
    assert!(ai.retreating());
    assert_ne!(command.buttons & ABILITY_2, 0);
    // Toward home at (1, 0, 1): -x.
    let step = walk(feet, &command);
    assert!(step[0] < feet[0]);
    hurt.tick = 11;
    assert_eq!(ai.command(&hurt).buttons & ABILITY_2, 0);
    let mut healed = senses(12, feet, &[], Goal::Objective(objective), None, &clear);
    healed.health = 0.9;
    let command = ai.command(&healed);
    assert!(!ai.retreating());
    assert!(walk(feet, &command)[0] > feet[0]);
}

/// Hunting (no objective), it walks at the nearest enemy; dead, it sends
/// no input.
#[test]
fn without_an_objective_an_ai_hunts_and_dead_it_idles() {
    let others = [enemy(4, [0.0, 0.0, 80.0]), enemy(5, [0.0, 0.0, -50.0])];
    let mut ai = AiController::new(Difficulty::Hard, 0, 1, 2);
    let command = ai.command(&senses(0, [0.0; 3], &others, Goal::Hunt, None, &clear));
    assert!(walk([0.0; 3], &command)[2] < 0.0);
    let mut dead = senses(1, [0.0; 3], &others, Goal::Hunt, None, &clear);
    dead.feet = None;
    let command = ai.command(&dead);
    assert_eq!((command.forward, command.right, command.buttons), (0, 0, 0));
}

#[test]
fn a_team_gets_each_roster_hero_once() {
    let mut random = Random::new(11);
    let picks = pick_heroes(5, &["Mei"], &mut random);
    assert_eq!(picks.len(), 5);
    assert!(!picks.contains(&"Mei"));
    for (index, hero) in picks.iter().enumerate() {
        assert!(ROSTER.contains(hero));
        assert!(!picks[index + 1..].contains(hero));
    }
    // More than the roster: it starts again.
    assert_eq!(pick_heroes(14, &[], &mut random).len(), 14);
}
