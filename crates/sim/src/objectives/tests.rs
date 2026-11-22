use super::*;
use crate::{
    match_flow::{CONTROL, ESCORT},
    mode_rules_data::{fixture, fixture_payload},
};
use lucy_content::maps::ObjectiveKind;

/// The rates 1.74's mode graphs send (`mode_graph_values` pins them to the
/// install): Assault and Hybrid, Control, Elimination.
pub(crate) const SIDED_RATES: CaptureRates = CaptureRates {
    capture: [1.0, 1.5, 2.0],
    decay: [1.0, 1.5, 2.0],
    decay_delay: 5.0,
};
pub(crate) const CONTROL_RATES: CaptureRates = CaptureRates {
    capture: [0.125, 0.1875, 0.25],
    decay: [0.125, 0.1875, 0.25],
    decay_delay: 5.0,
};
pub(crate) const ELIMINATION_RATES: CaptureRates = CaptureRates {
    capture: [1.0 / 3.0; 3],
    decay: [1.0 / 3.0; 3],
    decay_delay: 2.0,
};

/// The install's capture marks (`fixture` pins them to the data).
fn marks() -> [f64; 2] {
    fixture(ESCORT).capture_marks
}

const STEP: f64 = 1.0 / 60.0;

fn objective(position: [f64; 3]) -> MapObjective {
    MapObjective {
        kind: ObjectiveKind::Point,
        order: 0,
        name: Some("Point A".into()),
        placeable: 1,
        position,
    }
}

/// A yaw of `degrees` about the vertical axis.
fn yaw(degrees: f64) -> [f64; 4] {
    let half = degrees.to_radians() / 2.0;
    [0.0, half.sin(), 0.0, half.cos()]
}

/// A point at the origin with a 16 × 10 × 16 m box and base times.
pub(crate) fn point_data(capture: f64, decay: f64) -> PointData {
    PointData {
        placeable: 1,
        position: [0.0; 3],
        capture,
        decay,
        boxes: vec![CaptureBox {
            centre: [0.0, 4.0, 0.0],
            half: [8.0, 5.0, 8.0],
            rotation: yaw(0.0),
        }],
    }
}

fn run(seconds: f64, mut step: impl FnMut() -> bool) -> Option<f64> {
    let mut elapsed = 0.0;
    while elapsed < seconds {
        elapsed += STEP;
        if step() {
            return Some(elapsed);
        }
    }
    None
}

fn on(attackers: usize, defenders: usize) -> Presence {
    Presence {
        attackers,
        defenders,
    }
}

fn close(actual: f64, expected: f64, within: f64) {
    assert!(
        (actual - expected).abs() <= within,
        "{actual}, expected {expected}"
    );
}

/// A rotated box holds what lies within its half extents along its own
/// axes: a box 10 m long on x and 2 m deep on z, turned 90° about the
/// vertical, reaches 10 m along z and 2 m along x.
#[test]
fn a_rotated_box_contains_along_its_own_axes() {
    let area = CaptureBox {
        centre: [100.0, 5.0, -50.0],
        half: [10.0, 3.0, 2.0],
        rotation: yaw(90.0),
    };
    let at = |x: f64, y: f64, z: f64| area.contains([100.0 + x, 5.0 + y, -50.0 + z]);
    assert!(at(0.0, 0.0, 0.0));
    assert!(at(0.0, 0.0, 9.9) && at(0.0, 0.0, -9.9));
    assert!(at(1.9, 0.0, 0.0) && at(-1.9, 2.9, 0.0) && at(0.0, -2.9, 0.0));
    assert!(!at(2.1, 0.0, 0.0), "2 m deep along x once turned");
    assert!(!at(0.0, 0.0, 10.1));
    assert!(!at(0.0, 3.1, 0.0) && !at(0.0, -3.1, 0.0));
    // At 45° the corner direction is 10 m out along the box's long axis.
    let diagonal = CaptureBox {
        rotation: yaw(45.0),
        ..area
    };
    let along = std::f64::consts::FRAC_1_SQRT_2 * 9.9;
    assert!(diagonal.contains([100.0 + along, 5.0, -50.0 - along]));
    assert!(!diagonal.contains([100.0 + along, 5.0, -50.0 + along]));
}

/// A point's area is the union of its boxes; the payload's a sphere.
#[test]
fn areas_count_alive_players_inside_by_side() {
    let area = Area::Boxes(vec![
        CaptureBox {
            centre: [0.0, 2.0, 0.0],
            half: [5.0, 2.0, 5.0],
            rotation: yaw(0.0),
        },
        // An upper deck over one side.
        CaptureBox {
            centre: [4.0, 6.0, 0.0],
            half: [2.0, 2.0, 2.0],
            rotation: yaw(0.0),
        },
    ]);
    let player = |slot, team, position| Participant {
        slot,
        team,
        position,
    };
    let players = [
        player(0, 1, Some([4.0, 0.5, 4.0])),
        player(1, 1, Some([4.0, 7.0, 1.0])),
        player(2, 1, Some([0.0, 7.0, 0.0])),
        player(3, 0, Some([-4.9, 3.9, 0.0])),
        player(4, 0, None),
        player(5, 0, Some([5.1, 0.5, 0.0])),
    ];
    assert_eq!(area.presence(&players, 1), on(2, 1));
    assert_eq!(area.presence(&players, 0), on(1, 2));
    let sphere = Area::Sphere {
        centre: [0.0, 2.0, 0.0],
        radius: 7.0,
    };
    assert!(sphere.contains([0.0, 8.9, 0.0]) && sphere.contains([6.9, 2.0, 0.0]));
    assert!(!sphere.contains([0.0, -5.1, 0.0]) && !sphere.contains([5.0, 7.0, 0.0]));
}

/// 0E8E: base seconds over the mode's rate by attacker count: Anubis A's
/// 30 s gives 30/20/15, B's 18 s gives 18/12/9.
#[test]
fn point_capture_times_follow_the_base_time_and_the_attacker_count() {
    for (base, expected) in [(30.0, [30.0, 20.0, 15.0]), (18.0, [18.0, 12.0, 9.0])] {
        for (attackers, seconds) in [
            (1, expected[0]),
            (2, expected[1]),
            (3, expected[2]),
            (6, expected[2]),
        ] {
            let data = point_data(base, base);
            let mut point = CapturePoint::new(&objective([0.0; 3]), &data, SIDED_RATES, marks());
            let took = run(60.0, || point.step(STEP, 1.0, on(attackers, 0))).unwrap();
            close(took, seconds, 2.0 * STEP);
        }
    }
    // The time scale shortens it.
    let data = point_data(30.0, 30.0);
    let mut point = CapturePoint::new(&objective([0.0; 3]), &data, SIDED_RATES, marks());
    let took = run(60.0, || point.step(STEP, 0.1, on(1, 0))).unwrap();
    close(took, 3.0, 2.0 * STEP);
}

/// 0E8E: contested progress holds; without attackers it waits the delay
/// (5 s), then falls over the base decay seconds, faster with more
/// defenders on it, down to its last tick.
#[test]
fn a_point_decays_after_its_delay_by_defender_count_to_its_last_tick() {
    let data = point_data(30.0, 22.0);
    let fresh = || CapturePoint::new(&objective([0.0; 3]), &data, SIDED_RATES, marks());
    let mut point = fresh();
    run(15.0, || point.step(STEP, 1.0, on(1, 0)));
    close(point.progress(), 0.5, 0.001);
    assert_eq!(point.mark(), marks()[0]);
    run(20.0, || point.step(STEP, 1.0, on(2, 1)));
    close(point.progress(), 0.5, 0.001);
    // The delay runs from the last attacker leaving: 4.9 s of it pass.
    run(4.9, || point.step(STEP, 1.0, on(0, 0)));
    close(point.progress(), 0.5, 0.001);
    for (defenders, rate) in [
        (0, 1.0 / 22.0),
        (1, 1.0 / 22.0),
        (2, 1.5 / 22.0),
        (3, 2.0 / 22.0),
    ] {
        // 0.1 s more of the delay, then 1 s of decay.
        let mut point = point.clone();
        run(1.1, || point.step(STEP, 1.0, on(0, defenders)));
        close(
            point.progress(),
            0.5 - rate,
            0.01 * rate + 2.0 * STEP * rate,
        );
    }
    run(30.0, || point.step(STEP, 1.0, on(0, 0)));
    assert_eq!(point.progress(), marks()[0], "back to the first tick");
    assert!(!point.captured());
    // A returning attacker restarts the delay.
    let mut point = fresh();
    run(15.0, || point.step(STEP, 1.0, on(1, 0)));
    run(4.0, || point.step(STEP, 1.0, on(0, 0)));
    point.step(STEP, 1.0, on(1, 1));
    run(4.0, || point.step(STEP, 1.0, on(0, 0)));
    close(point.progress(), 0.5, 0.001);
}

/// The marks are the rules': with others, the point keeps those.
#[test]
fn a_point_falls_back_to_the_marks_it_is_given() {
    let data = point_data(30.0, 30.0);
    let mut point = CapturePoint::new(&objective([0.0; 3]), &data, SIDED_RATES, [0.25, 0.75]);
    run(10.0, || point.step(STEP, 1.0, on(1, 0)));
    assert_eq!(point.mark(), 0.25);
    run(40.0, || point.step(STEP, 1.0, on(0, 0)));
    assert_eq!(point.progress(), 0.25);
}

/// Straight waypoints along x at `stops` (stored length = arc), each
/// `(distance, barrier, event)`.
fn straight(stops: &[(f64, bool, Option<WaypointEvent>)]) -> Vec<WaypointData> {
    stops
        .iter()
        .enumerate()
        .map(|(k, &(at, barrier, event))| {
            let next = stops.get(k + 1).map(|stop| stop.0);
            WaypointData {
                placeable: 100 + k as u32,
                position: [at, 0.0, 0.0],
                curve: next.map(|next| [[0.0; 3], [0.0; 3], [next - at, 0.0, 0.0]]),
                length: next.map(|next| next - at),
                barrier,
                event,
            }
        })
        .collect()
}

/// A track is its Bézier segments, measured in their stored lengths: a
/// quarter circle stored as 10 m runs 10 m of track evenly along its arc.
#[test]
fn a_track_follows_its_curves_by_stored_length() {
    // A cubic close to a quarter circle of radius 10 from (10, 0, 0) to
    // (0, 0, 10): handles 0.5523 r long.
    let k = 0.5523 * 10.0;
    let [p0, p1, p2, p3] = [
        [10.0, 0.0, 0.0],
        [10.0, 0.0, k],
        [k, 0.0, 10.0],
        [0.0, 0.0, 10.0],
    ];
    let coefficient = |f: fn(f64, f64, f64, f64) -> f64| -> [f64; 3] {
        std::array::from_fn(|axis| f(p0[axis], p1[axis], p2[axis], p3[axis]))
    };
    let a = coefficient(|p0, p1, p2, p3| -p0 + 3.0 * p1 - 3.0 * p2 + p3);
    let b = coefficient(|p0, p1, p2, _| 3.0 * p0 - 6.0 * p1 + 3.0 * p2);
    let c = coefficient(|p0, p1, _, _| 3.0 * (p1 - p0));
    let waypoints = vec![
        WaypointData {
            placeable: 1,
            position: p0,
            curve: Some([a, b, c]),
            length: Some(10.0),
            barrier: false,
            event: None,
        },
        WaypointData {
            placeable: 2,
            position: p3,
            curve: Some([[0.0; 3], [0.0; 3], [0.0, 0.0, 5.0]]),
            length: Some(5.0),
            barrier: true,
            event: None,
        },
        WaypointData {
            placeable: 3,
            position: [0.0, 0.0, 15.0],
            curve: None,
            length: None,
            barrier: false,
            event: None,
        },
    ];
    let track = Track::new(&waypoints).unwrap();
    assert_eq!(track.length(), 15.0);
    let distances: Vec<f64> = track.waypoints().iter().map(|w| w.distance).collect();
    assert_eq!(distances, [0.0, 10.0, 15.0]);
    let at = |distance: f64| track.position(distance);
    let near = |a: [f64; 3], b: [f64; 3]| distance(a, b) < 0.05;
    assert!(near(at(0.0), p0) && near(at(10.0), p3) && near(at(15.0), [0.0, 0.0, 15.0]));
    // Halfway along the stored length is halfway along the arc: 45°.
    let half = 10.0 * std::f64::consts::FRAC_1_SQRT_2;
    assert!(near(at(5.0), [half, 0.0, half]), "{:?}", at(5.0));
    assert!(near(at(12.5), [0.0, 0.0, 12.5]));
    // A curve that misses the next waypoint is refused.
    let mut broken = waypoints.clone();
    broken[1].position = [0.0, 0.0, 11.0];
    assert!(Track::new(&broken).is_err());
}

fn payload_data(track: Vec<WaypointData>, speed: f64) -> PayloadData {
    PayloadData {
        placeable: 50,
        speed,
        unlock: [0.0, 0.0],
        auto_start: false,
        track: Track::new(&track).unwrap(),
    }
}

/// 1225: 1.5 / 1.75 / 2 m/s times the map's factor, capped at three
/// pushers; any defender in range stops it, for 1 s after he leaves; after
/// 10 s without an attacker it rolls back at 0.3 m/s to the last barrier.
#[test]
fn the_payload_moves_by_pushers_contests_and_rolls_back_to_its_last_barrier() {
    let rules = fixture_payload();
    let data = payload_data(
        straight(&[
            (0.0, false, None),
            (20.0, true, None),
            (30.0, true, None),
            (40.0, false, None),
        ]),
        1.1,
    );
    let mut payload = Payload::new(&data, rules, &[103], 1.0).unwrap();
    assert_eq!(payload.length(), 40.0);
    for (pushers, speed) in [(1, 1.65), (2, 1.925), (3, 2.2), (5, 2.2)] {
        let mut payload = payload.clone();
        run(1.0, || payload.step(STEP, 1.0, on(pushers, 0)).is_some());
        close(payload.distance(), speed, 0.03);
    }
    run(15.0, || payload.step(STEP, 1.0, on(1, 0)).is_some());
    let pushed = payload.distance();
    close(pushed, 15.0 * 1.65, 0.05);
    run(5.0, || payload.step(STEP, 1.0, on(3, 1)).is_some());
    assert_eq!(payload.distance(), pushed, "a defender stops it");
    run(0.9, || payload.step(STEP, 1.0, on(3, 0)).is_some());
    assert_eq!(payload.distance(), pushed, "the contest lasts 1 s more");
    run(0.2, || payload.step(STEP, 1.0, on(3, 0)).is_some());
    assert!(payload.distance() > pushed);
    run(2.0, || payload.step(STEP, 1.0, on(1, 0)).is_some());
    let pushed = payload.distance();
    assert!(pushed > 20.0);
    run(9.9, || payload.step(STEP, 1.0, on(0, 2)).is_some());
    assert_eq!(payload.distance(), pushed, "no rollback before 10 s");
    run(1.1, || payload.step(STEP, 1.0, on(0, 0)).is_some());
    close(pushed - payload.distance(), rules.rollback_speed, 0.03);
    run(60.0, || payload.step(STEP, 1.0, on(0, 0)).is_some());
    assert_eq!(
        payload.distance(),
        20.0,
        "rolls back to the barrier at 20 m"
    );
    assert!(payload.best() > 20.0);
    assert!(distance(payload.position(), [20.0, 0.0, 0.0]) < 1e-9);
    let area = payload.area();
    assert!(area.contains([20.0, 8.9, 0.0]) && !area.contains([20.0, -5.1, 0.0]));
    let delivered = run(60.0, || payload.step(STEP, 1.0, on(2, 0)) == Some(1));
    assert!(delivered.is_some() && payload.delivered());
}

/// 121E: a waypoint's event stops the payload for its pause; with
/// auto_next it then runs alone at 2 m/s to the next event, whoever is
/// near. A checkpoint is counted when the payload reaches it.
#[test]
fn checkpoints_pause_the_payload_and_events_send_it_on_alone() {
    let rules = fixture_payload();
    let pause = |pause, auto_next| Some(WaypointEvent { pause, auto_next });
    let data = payload_data(
        straight(&[
            (0.0, false, None),
            (10.0, true, pause(6.0, false)),
            (15.0, true, pause(5.0, true)),
            (20.0, true, None),
            (25.0, true, pause(0.0, false)),
            (40.0, false, pause(0.0, false)),
        ]),
        1.0,
    );
    let mut payload = Payload::new(&data, rules, &[101, 105], 1.0).unwrap();
    let reached = run(30.0, || payload.step(STEP, 1.0, on(1, 0)) == Some(1)).unwrap();
    close(reached, 10.0 / 1.5, 2.0 * STEP);
    assert_eq!(payload.distance(), 10.0);
    assert!(!payload.delivered());
    // How long it takes for the distance to pass `at` under `presence`.
    let until = |payload: &mut Payload, at: f64, presence| {
        run(30.0, || {
            payload.step(STEP, 1.0, presence);
            payload.distance() >= at
        })
        .unwrap()
    };
    // The checkpoint's 6 s pause, then 5 m pushed at 1.5 m/s.
    let moved = until(&mut payload, 10.0 + 1e-6, on(3, 0));
    close(moved, 6.0, 2.0 * STEP);
    close(until(&mut payload, 15.0, on(1, 0)), 5.0 / 1.5, 2.0 * STEP);
    // Stopped for 5 s at 15 m, then alone at 2 m/s to the event at 25 m,
    // with only defenders near.
    let moved = until(&mut payload, 15.0 + 1e-6, on(0, 1));
    close(moved, 5.0, 2.0 * STEP);
    close(until(&mut payload, 25.0, on(0, 1)), 10.0 / 2.0, 2.0 * STEP);
    run(5.0, || payload.step(STEP, 1.0, on(0, 1)).is_some());
    assert_eq!(payload.distance(), 25.0, "the run ends at the next event");
    let delivered = run(30.0, || payload.step(STEP, 1.0, on(2, 0)) == Some(2));
    assert!(delivered.is_some() && payload.delivered());
}

/// 1225: once active a Hybrid payload waits v2102 + v2118, then with v1362
/// runs alone to its first event (King's Row's unlock run).
#[test]
fn a_hybrid_payload_waits_then_runs_its_unlock_section() {
    let rules = fixture_payload();
    let mut data = payload_data(
        straight(&[
            (0.0, false, None),
            (
                8.0,
                true,
                Some(WaypointEvent {
                    pause: 0.0,
                    auto_next: false,
                }),
            ),
            (30.0, false, None),
        ]),
        1.0,
    );
    data.unlock = [4.0, 5.0];
    data.auto_start = true;
    let mut payload = Payload::new(&data, rules, &[102], 1.0).unwrap();
    run(8.9, || payload.step(STEP, 1.0, on(0, 0)).is_some());
    assert_eq!(payload.distance(), 0.0, "9 s of waits");
    let ran = run(10.0, || {
        payload.step(STEP, 1.0, on(0, 0));
        payload.distance() >= 8.0
    })
    .unwrap();
    close(ran, 0.1 + 8.0 / 2.0, 2.0 * STEP);
    run(10.0, || payload.step(STEP, 1.0, on(0, 0)).is_some());
    assert_eq!(payload.distance(), 8.0);
    // A checkpoint that isn't on the track has no payload.
    assert!(Payload::new(&data, rules, &[7], 1.0).is_none());
}

#[test]
fn control_unlocks_captures_scores_and_holds_at_99_while_contested() {
    let point_at = [0.0; 3];
    let control = fixture(CONTROL).control;
    let data = point_data(1.0, 1.0);
    let mut point = ControlPoint::new(&objective(point_at), &data, 30.0, CONTROL_RATES, control);
    let blue = Participant {
        slot: 0,
        team: 0,
        position: Some(point_at),
    };
    let red = Participant {
        slot: 1,
        team: 1,
        position: Some(point_at),
    };
    run(29.0, || point.step(STEP, 1.0, &[blue]).is_some());
    assert!(point.locked());
    run(1.0 + 7.9, || point.step(STEP, 1.0, &[blue]).is_some());
    assert_eq!(point.owner(), None, "8 s with one player");
    run(0.2, || point.step(STEP, 1.0, &[blue]).is_some());
    assert_eq!(point.owner(), Some(0));
    // 118.8 s more takes blue to 99%, held there while red contests.
    let won = run(200.0, || point.step(STEP, 1.0, &[blue, red]).is_some());
    assert!(won.is_none());
    assert_eq!(point.percent(0), 99.0);
    assert!(point.contested(&[blue, red]));
    // Red alone takes it in 8 s; blue keeps its 99%.
    run(8.1, || point.step(STEP, 1.0, &[red]).is_some());
    assert_eq!(point.owner(), Some(1));
    assert_eq!(point.percent(0), 99.0);
    let won = run(121.0, || point.step(STEP, 1.0, &[red]).is_some()).unwrap();
    close(won, 120.0, 0.5);
    assert_eq!(point.percent(1), 100.0);
}

/// The score rate and the overtime hold are the rules'.
#[test]
fn control_scores_at_the_rate_and_holds_at_the_percentage_it_is_given() {
    let scoring = ControlScoring {
        percent_per_second: 2.0,
        hold_at: 90.0,
    };
    let data = point_data(1.0, 1.0);
    let mut point = ControlPoint::new(&objective([0.0; 3]), &data, 0.0, CONTROL_RATES, scoring);
    let blue = Participant {
        slot: 0,
        team: 0,
        position: Some([0.0; 3]),
    };
    let red = Participant { team: 1, ..blue };
    run(8.0 + STEP, || point.step(STEP, 1.0, &[blue]).is_some());
    assert_eq!(point.owner(), Some(0));
    run(10.0, || point.step(STEP, 1.0, &[blue]).is_some());
    close(point.percent(0), 20.0, 0.2);
    run(60.0, || point.step(STEP, 1.0, &[blue, red]).is_some());
    assert_eq!(point.percent(0), 90.0);
}

#[test]
fn control_capture_time_follows_the_player_count() {
    let point_at = [0.0; 3];
    let data = point_data(1.0, 1.0);
    for (players, seconds) in [(1, 8.0), (2, 16.0 / 3.0), (3, 4.0), (4, 4.0)] {
        let control = fixture(CONTROL).control;
        let mut point = ControlPoint::new(&objective(point_at), &data, 0.0, CONTROL_RATES, control);
        let team: Vec<_> = (0..players)
            .map(|slot| Participant {
                slot,
                team: 0,
                position: Some(point_at),
            })
            .collect();
        run(seconds - 0.1, || point.step(STEP, 1.0, &team).is_some());
        assert_eq!(point.owner(), None, "{players} players");
        run(0.2, || point.step(STEP, 1.0, &team).is_some());
        assert_eq!(point.owner(), Some(0), "{players} players");
    }
}

/// 0E8E on a Control point: an unfinished capture decays to 0 once its
/// team has been off the point for 5 s, at once when the other team stands
/// on a neutral point, which it takes only once the decay is over.
#[test]
fn an_unfinished_control_capture_decays_to_zero() {
    let control = fixture(CONTROL).control;
    let data = point_data(1.0, 1.0);
    let fresh = || ControlPoint::new(&objective([0.0; 3]), &data, 0.0, CONTROL_RATES, control);
    let blue = Participant {
        slot: 0,
        team: 0,
        position: Some([0.0; 3]),
    };
    let red = Participant {
        slot: 1,
        team: 1,
        ..blue
    };
    // Blue takes 50% in 4 s, steps off: 5 s later it falls in 8 s per 100%.
    let mut point = fresh();
    run(4.0, || point.step(STEP, 1.0, &[blue]).is_some());
    run(4.9 + 3.9, || point.step(STEP, 1.0, &[]).is_some());
    run(4.0, || point.step(STEP, 1.0, &[blue]).is_some());
    assert_eq!(point.owner(), None, "the decay took about half");
    let took = run(10.0, || {
        point.step(STEP, 1.0, &[blue]);
        point.owner().is_some()
    })
    .unwrap();
    assert!(took > 3.0 && took < 4.5, "{took}");
    // Red on a neutral point blue half took: 4 s of decay, then its own 8 s.
    let mut point = fresh();
    run(4.0, || point.step(STEP, 1.0, &[blue]).is_some());
    let took = run(20.0, || {
        point.step(STEP, 1.0, &[red]);
        point.owner().is_some()
    })
    .unwrap();
    close(took, 12.0, 0.1);
    assert_eq!(point.owner(), Some(1));
}

/// Elimination's point: 3 s for one player or many (0D0B's single rate).
#[test]
fn the_elimination_point_takes_three_seconds() {
    let data = point_data(1.0, 1.0);
    for players in [1, 3] {
        let mut point = ControlPoint::tiebreak(&data, ELIMINATION_RATES, 0.0);
        let team: Vec<_> = (0..players)
            .map(|slot| Participant {
                slot,
                team: 1,
                position: Some([1.0, 0.0, 1.0]),
            })
            .collect();
        let took = run(10.0, || {
            point.step(STEP, 1.0, &team);
            point.owner().is_some()
        })
        .unwrap();
        close(took, 3.0, 2.0 * STEP);
    }
}
