use super::*;
use crate::{
    mode_rules_data::fixture,
    objective_data::{ObjectiveTimes, PayloadData, PointData, WaypointData, WaypointEvent},
    objectives::{
        CaptureBox, Track,
        tests::{CONTROL_RATES, ELIMINATION_RATES, SIDED_RATES},
    },
};
use lucy_content::maps::MapObjective;

const STEP: f64 = 1.0 / 62.5;
const A: [f64; 3] = [100.0, 0.0, 0.0];
const B: [f64; 3] = [200.0, 0.0, 0.0];
const SPAWN: [f64; 3] = [0.0; 3];

/// The mode graphs' values in 104319; `mode_graph_values` pins them to the
/// install in a game-tier test.
const VALUES: ModeGraphValues = ModeGraphValues {
    sided_round_end: 11.6,
    control_round_end: 10.9,
    elimination_round_end: 2.3,
    fuse: crate::mode_graph_values::Fuse {
        full: 5.0,
        steady: 15.0,
        shortening: 15.0,
        floor: 0.3,
    },
    sided_capture: SIDED_RATES,
    control_capture: CONTROL_RATES,
    elimination_capture: ELIMINATION_RATES,
};

fn objective(
    kind: ObjectiveKind,
    order: u32,
    name: Option<&str>,
    position: [f64; 3],
) -> MapObjective {
    MapObjective {
        kind,
        order,
        name: name.map(Into::into),
        placeable: 100 + order,
        position,
    }
}

fn map(mode: u64, objectives: Vec<MapObjective>) -> GameMap {
    GameMap {
        name: "Fixture".into(),
        mode_guid: mode,
        team_sizes: vec![6, 6],
        objectives,
        ..GameMap::practice_range()
    }
}

/// Hybrid: point A, then the payload from the origin through its
/// checkpoints 40 m and 60 m out.
fn hybrid() -> GameMap {
    let mut map = escort();
    map.mode_guid = HYBRID;
    map.objectives[0] = objective(ObjectiveKind::Point, 0, Some("Point A"), A);
    map
}

/// The objective data of a fixture map, like the install's: each point a
/// 16 × 10 × 16 m box over its position, Hybrid's point 32 s, Control 1 s
/// at Control's rates; Escort and Hybrid 4:00, +2:30, +1:30 with
/// Competitive rules and 5:00, +3:00, +2:00 without; the payload on a
/// straight track through its checkpoints at Gibraltar's speed factor
/// (1.1), its first checkpoint pausing it 7.5 s, Hybrid's waiting King's
/// Row's 4 + 5 s once A falls. An Elimination map has its point at its
/// spawn.
fn objectives(map: &GameMap) -> Arc<MapObjectives> {
    let mode = Mode::of(map.mode_guid);
    let point = |placeable: u32, position: [f64; 3], capture: f64, decay: f64| PointData {
        placeable,
        position,
        capture,
        decay,
        boxes: vec![CaptureBox {
            centre: [position[0], position[1] + 4.0, position[2]],
            half: [8.0, 5.0, 8.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
        }],
    };
    let mut data = MapObjectives::default();
    for objective in &map.objectives {
        let times = |competitive, competitive_extra, quick_play| ObjectiveTimes {
            placeable: objective.placeable,
            objective: objective.order,
            competitive,
            competitive_extra,
            quick_play,
        };
        let place = objective.placeable;
        match (objective.kind, mode) {
            (ObjectiveKind::Point, _) => {
                data.points
                    .push(point(place, objective.position, 32.0, 32.0));
                data.objectives.push(times(240.0, 0.0, 300.0));
            }
            (ObjectiveKind::Checkpoint, _) => data.objectives.push(match objective.order {
                0 => times(240.0, 0.0, 300.0),
                1 => times(150.0, 0.0, 180.0),
                _ => times(90.0, 0.0, 120.0),
            }),
            (ObjectiveKind::ControlPoint, _) => {
                data.points.push(point(place, objective.position, 1.0, 1.0));
            }
            (ObjectiveKind::Payload, _) => {
                let start = WaypointData {
                    placeable: place + 1000,
                    position: objective.position,
                    curve: None,
                    length: None,
                    barrier: false,
                    event: None,
                };
                let mut track = vec![start];
                for (k, checkpoint) in map.objectives_of(ObjectiveKind::Checkpoint).enumerate() {
                    let pause = if k == 0 { 7.5 } else { 0.0 };
                    track.push(WaypointData {
                        placeable: checkpoint.placeable,
                        position: checkpoint.position,
                        curve: None,
                        length: None,
                        barrier: true,
                        event: Some(WaypointEvent {
                            pause,
                            auto_next: false,
                        }),
                    });
                }
                for k in 0..track.len() - 1 {
                    let [from, to] = [track[k].position, track[k + 1].position];
                    let step: [f64; 3] = std::array::from_fn(|axis| to[axis] - from[axis]);
                    track[k].curve = Some([[0.0; 3], [0.0; 3], step]);
                    track[k].length = Some(step.iter().map(|part| part * part).sum::<f64>().sqrt());
                }
                let hybrid = mode == Some(Mode::Hybrid);
                data.payloads.push(PayloadData {
                    placeable: place,
                    speed: 1.1,
                    unlock: if hybrid { [4.0, 5.0] } else { [0.0, 0.0] },
                    auto_start: false,
                    track: Track::new(&track).unwrap(),
                });
            }
        }
    }
    if mode == Some(Mode::Elimination) {
        let centre = map.spawns[0].position;
        data.points.push(point(900, centre, 1.0, 1.0));
    }
    Arc::new(data)
}

/// The map's flow by the graph values, its mode's rules and its objective
/// data as the install has them.
fn new_flow(map: &GameMap, competitive: bool, timers: Timers) -> Option<MatchFlow> {
    MatchFlow::new(
        map,
        competitive,
        VALUES,
        fixture(map.mode_guid),
        objectives(map),
        timers,
    )
}

fn at(slot: usize, team: u8, position: [f64; 3]) -> Participant {
    Participant {
        slot,
        team,
        position: Some(position),
    }
}

/// Run until `until` holds for an event or `seconds` pass; the time it
/// took and every event on the way.
struct Run {
    flow: MatchFlow,
    elapsed: f64,
    events: Vec<(f64, FlowEvent)>,
}

impl Run {
    fn new(flow: MatchFlow) -> Self {
        Self {
            flow,
            elapsed: 0.0,
            events: Vec::new(),
        }
    }

    fn until(
        &mut self,
        seconds: f64,
        players: &[Participant],
        until: impl Fn(&FlowEvent) -> bool,
    ) -> Option<f64> {
        let start = self.elapsed;
        while self.elapsed - start < seconds {
            self.elapsed += STEP;
            let events = self.flow.advance(STEP, players);
            let hit = events.iter().any(&until);
            self.events
                .extend(events.into_iter().map(|event| (self.elapsed, event)));
            if hit {
                return Some(self.elapsed - start);
            }
        }
        None
    }

    fn phase(&mut self, seconds: f64, players: &[Participant], phase: Phase) -> f64 {
        self.until(seconds, players, |event| *event == FlowEvent::Phase(phase))
            .unwrap_or_else(|| panic!("no {phase:?} within {seconds} s"))
    }

    fn phases(&self) -> Vec<Phase> {
        self.events
            .iter()
            .filter_map(|(_, event)| match event {
                FlowEvent::Phase(phase) => Some(*phase),
                _ => None,
            })
            .collect()
    }

    fn messages(&self) -> Vec<u64> {
        self.events
            .iter()
            .filter_map(|(_, event)| match event {
                FlowEvent::Message { message, .. } => Some(*message),
                _ => None,
            })
            .filter(|&message| message != PHASE_MESSAGE)
            .collect()
    }
}

fn close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= 2.0 * STEP,
        "{actual} s, expected {expected} s"
    );
}

#[test]
fn modes_without_a_flow_or_their_objectives_get_none() {
    let none = |map: &GameMap| new_flow(map, false, Timers::default()).is_none();
    assert!(none(&GameMap::practice_range()));
    assert!(none(&map(HYBRID, Vec::new())));
    assert!(none(&map(ESCORT, Vec::new())));
    assert!(none(&map(CONTROL, Vec::new())));
    let mut skirmish = new_flow(&map(SKIRMISH, Vec::new()), false, Timers::default()).unwrap();
    assert_eq!(skirmish.mode(), Mode::Skirmish);
    // No assembly: Skirmish's round begins with the players (`0D76` sets
    // no phase).
    skirmish.start();
    assert_eq!(skirmish.phase(), Phase::Round);
    assert!(!skirmish.assembling());
    assert_eq!(Mode::of(ASSAULT_TEST), Some(Mode::Assault));
    assert_eq!(Mode::of(BOUNTY_HUNTER), Some(Mode::Deathmatch));
}

/// Assault plays on its mode graph `0F4A` alone: its flow, objectives or
/// none, only follows the graph ([`MatchFlow::follow`]); started, it stays
/// waiting and plays nothing. The graph's winner decides the match, and
/// the players go home only once the graph ends the game (`180A`'s
/// `STU_147B34C6` after the match end's cards), not on a timer.
#[test]
fn an_assault_flow_only_follows_its_mode_graph() {
    let mut flow = new_flow(&map(ASSAULT, Vec::new()), false, Timers::default()).unwrap();
    assert_eq!(flow.mode(), Mode::Assault);
    flow.start();
    assert_eq!(flow.phase(), Phase::Waiting);
    assert!(flow.advance(600.0, &[at(0, 1, A)]).is_empty());
    assert_eq!(flow.attackers(), None);
    let state = MatchState {
        phase: Phase::Round,
        clock: 200.0,
        winner: None,
        attackers: Some(1),
        objective: 1,
        overtime: false,
        over: false,
    };
    assert_eq!(flow.follow(state), [FlowEvent::Phase(Phase::Round)]);
    assert_eq!(flow.clock(), 200.0);
    assert_eq!(flow.attackers(), Some(1));
    let ended = MatchState {
        phase: Phase::MatchEnd,
        clock: 0.0,
        winner: Some(0),
        ..state
    };
    assert_eq!(
        flow.follow(ended),
        [FlowEvent::MatchOver, FlowEvent::Phase(Phase::MatchEnd)]
    );
    assert_eq!(flow.standing(0, 0), Some(Standing::Won));
    for _ in 0..10_000 {
        assert!(flow.follow(ended).is_empty(), "the cards still show");
    }
    assert_eq!(
        flow.follow(MatchState {
            over: true,
            ..ended
        }),
        [FlowEvent::Finished]
    );
}

#[test]
fn quick_play_hybrid_runs_its_phases_and_defenders_win_by_holding_a() {
    let mut run = Run::new(new_flow(&hybrid(), false, Timers::default()).unwrap());
    assert_eq!(run.flow.phase(), Phase::Waiting);
    assert!(
        run.flow.advance(10.0, &[]).is_empty(),
        "waiting for players"
    );
    run.flow.start();
    assert!(run.flow.assembling());
    assert_eq!(run.flow.variables()[&CLOCK], Value::Float(30.0));
    assert_eq!(
        run.flow.variables()[&BANNER],
        Value::Asset(0x0de0_0000_0000_6114)
    );
    assert_eq!(run.flow.variables()[&ATTACKERS], Value::Integer(1));
    assert_eq!(run.flow.variables()[&DEFENDERS], Value::Integer(0));
    assert_eq!(
        run.flow.variables()[&OBJECTIVE_NAME],
        Value::String("Point A".into())
    );
    let attacker = at(0, 1, SPAWN);
    let defender = at(1, 0, A);
    close(run.phase(40.0, &[attacker, defender], Phase::Setup), 30.0);
    close(run.phase(60.0, &[attacker, defender], Phase::Round), 45.0);
    assert_eq!(run.flow.clock(), 300.0);
    assert!(!run.flow.assembling());
    // A with a defender on it holds.
    let contested = [at(0, 1, A), defender];
    assert!(run.until(20.0, &contested, |_| true).is_none());
    assert_eq!(run.flow.objective_progress(), Some(0.0));
    // Time runs out with no attacker on A: no overtime, the defenders win.
    let over = run
        .until(300.0, &[attacker, defender], |event| {
            *event == FlowEvent::MatchOver
        })
        .unwrap();
    close(over, 280.0);
    assert_eq!(run.flow.phase(), Phase::RoundEnd);
    assert_eq!(run.flow.standing(0, 0), Some(Standing::Won));
    assert_eq!(run.flow.standing(0, 1), Some(Standing::Lost));
    close(
        run.phase(20.0, &[], Phase::MatchEnd),
        VALUES.sided_round_end,
    );
    let finished = run
        .until(30.0, &[], |event| *event == FlowEvent::Finished)
        .unwrap();
    close(finished, MATCH_END);
    assert_eq!(
        run.phases(),
        [
            Phase::Assemble,
            Phase::Setup,
            Phase::Round,
            Phase::RoundEnd,
            Phase::MatchEnd
        ]
    );
    assert_eq!(
        run.messages(),
        [
            SETUP_MESSAGE,
            ROUND_MESSAGE,
            ROUND_END_MESSAGE,
            MATCH_END_MESSAGE
        ]
    );
    let winner = run.events.iter().find_map(|(_, event)| match event {
        FlowEvent::Message {
            message: MATCH_END_MESSAGE,
            parameters,
        } => Some(parameters[&WINNER].clone()),
        _ => None,
    });
    assert_eq!(winner, Some(Value::Integer(0)));
    assert!(!run.flow.respawns());
}

/// C32S2's "Match time" rule: Set Match Time(1) at 0.25 s and at 1.35 s
/// ends assembling heroes and then setup within 2.4 s, and Set Match
/// Time(3600) gives the round 3599 s (the action's limit). Before the
/// players are in, nothing.
#[test]
fn set_match_time_gives_the_phase_its_seconds() {
    let set = |seconds: f64| Variables::from([(SETUP_CLOCK, Value::Float(seconds))]);
    let mut run = Run::new(new_flow(&hybrid(), false, Timers::default()).unwrap());
    run.flow.receive(SET_MATCH_TIME_MESSAGE, &set(1.0));
    run.flow.start();
    assert_eq!(run.flow.clock(), 30.0);
    run.until(0.25, &[], |_| false);
    run.flow.receive(SET_MATCH_TIME_MESSAGE, &set(1.0));
    close(run.phase(2.0, &[], Phase::Setup), 1.0);
    run.until(0.1, &[], |_| false);
    run.flow.receive(SET_MATCH_TIME_MESSAGE, &set(1.0));
    close(run.phase(2.0, &[], Phase::Round), 1.0);
    assert!(run.elapsed < 2.45, "the round at {} s", run.elapsed);
    run.flow.receive(SET_MATCH_TIME_MESSAGE, &set(3600.0));
    assert_eq!(run.flow.clock(), 3599.0);
    // Another message to the mode entity leaves the clock.
    run.flow.receive(SETUP_MESSAGE, &set(5.0));
    assert_eq!(run.flow.clock(), 3599.0);
}

#[test]
fn the_time_scale_shortens_every_timer_and_capture() {
    let timers = Timers { scale: 0.1 };
    let mut run = Run::new(new_flow(&hybrid(), false, timers).unwrap());
    run.flow.start();
    let attacker = [at(0, 1, A)];
    close(run.phase(10.0, &attacker, Phase::Setup), 3.0);
    close(run.phase(10.0, &attacker, Phase::Round), 4.5);
    close(run.flow.clock(), 30.0);
    let took = run
        .until(10.0, &attacker, |event| *event == FlowEvent::Objective(0))
        .unwrap();
    close(took, 3.2);
}

/// The fuse (0F4A): 5 s for the first 15 s of overtime, then it falls
/// evenly to 1.5 s at 30 s. Before the graph was read the server burned
/// 5 s, then 3 s after 20 s, then 1.5 s after 30 s.
#[test]
fn time_out_with_an_attacker_on_the_point_is_overtime_until_the_fuse_burns() {
    for (held, fuse) in [(10.0, 5.0), (25.0, 8.0 / 3.0), (40.0, 1.5)] {
        let mut run = Run::new(new_flow(&hybrid(), false, Timers::default()).unwrap());
        run.flow.start();
        let away = [at(0, 1, SPAWN)];
        run.phase(100.0, &away, Phase::Round);
        // An attacker contested by a defender when time runs out.
        let contested = [at(0, 1, A), at(1, 0, A)];
        let overtime = run
            .until(400.0, &contested, |event| {
                *event == FlowEvent::Overtime(true)
            })
            .unwrap();
        close(overtime, 300.0);
        assert!(run.flow.in_overtime());
        assert_eq!(run.flow.clock(), 0.0);
        // Held as long as he stays.
        assert!(run.until(held, &contested, |_| true).is_none());
        // He leaves: the fuse the overtime has reached burns.
        let ended = run
            .until(10.0, &away, |event| *event == FlowEvent::MatchOver)
            .unwrap();
        close(ended, fuse);
        assert_eq!(
            run.flow.standing(0, 0),
            Some(Standing::Won),
            "defenders hold"
        );
    }
}

#[test]
fn a_taken_in_overtime_goes_on_with_three_minutes() {
    let mut run = Run::new(new_flow(&hybrid(), false, Timers::default()).unwrap());
    run.flow.start();
    run.phase(100.0, &[], Phase::Round);
    // He reaches A with 10 s left and takes it alone in 32 s.
    run.until(290.0, &[at(0, 1, SPAWN)], |_| false);
    let taken = run
        .until(40.0, &[at(0, 1, A)], |event| {
            *event == FlowEvent::Objective(0)
        })
        .unwrap();
    close(taken, 32.0);
    assert!(!run.flow.in_overtime());
    close(run.flow.clock(), 180.0);
}

#[test]
fn competitive_hybrid_swaps_sides_and_the_farther_attack_wins() {
    let mut run = Run::new(new_flow(&hybrid(), true, Timers::default()).unwrap());
    run.flow.start();
    run.phase(100.0, &[], Phase::Round);
    // Team 1 takes A, then runs out of time.
    run.until(40.0, &[at(0, 1, A)], |event| {
        *event == FlowEvent::Objective(0)
    });
    run.until(500.0, &[at(0, 1, SPAWN)], |event| {
        *event == FlowEvent::Phase(Phase::RoundEnd)
    })
    .unwrap();
    assert!(run.flow.standing(0, 0).is_none(), "not decided yet");
    // Hero select before the second round is 25 s (rule 2), then setup.
    close(
        run.phase(20.0, &[], Phase::Assemble),
        VALUES.sided_round_end,
    );
    assert_eq!(run.flow.attackers(), Some(0));
    close(run.phase(30.0, &[], Phase::Setup), 25.0);
    run.phase(50.0, &[], Phase::Round);
    // Team 0 takes a tick of A only: team 1 wins.
    run.until(12.0, &[at(0, 0, A)], |_| false);
    run.until(500.0, &[at(0, 0, SPAWN)], |event| {
        *event == FlowEvent::MatchOver
    })
    .unwrap();
    assert_eq!(run.flow.standing(3, 1), Some(Standing::Won));
    assert_eq!(run.flow.standing(3, 0), Some(Standing::Lost));
}

#[test]
fn competitive_hybrid_with_equal_progress_is_a_draw() {
    let mut run = Run::new(new_flow(&hybrid(), true, Timers::default()).unwrap());
    run.flow.start();
    for team in [1, 0] {
        run.phase(200.0, &[], Phase::Round);
        // Both get 12 s on A: one tick each (progress below a tick doesn't count).
        run.until(12.0, &[at(0, team, A)], |_| false);
        run.until(500.0, &[], |event| {
            matches!(event, FlowEvent::Phase(Phase::RoundEnd))
        })
        .unwrap();
    }
    assert_eq!(run.flow.standing(0, 0), Some(Standing::Drew));
    assert_eq!(run.flow.standing(0, 1), Some(Standing::Drew));
}

fn escort() -> GameMap {
    let mut map = map(
        ESCORT,
        vec![
            objective(ObjectiveKind::Checkpoint, 0, None, [20.0, 0.0, 0.0]),
            objective(ObjectiveKind::Checkpoint, 1, None, [40.0, 0.0, 0.0]),
            objective(ObjectiveKind::Checkpoint, 2, None, [60.0, 0.0, 0.0]),
            objective(ObjectiveKind::Payload, 0, None, [0.0, 0.0, 0.0]),
        ],
    );
    map.name = "Watchpoint: Gibraltar".into();
    map
}

/// Escort: 5:00 without Competitive rules, the checkpoints +3:00 and
/// +2:00; with them 4:00, +2:30 and +1:30 (each checkpoint's own times).
#[test]
fn escort_checkpoints_add_time_and_the_delivery_wins() {
    for (competitive, start, bonus) in [(false, 300.0, 180.0), (true, 240.0, 150.0)] {
        let mut run = Run::new(new_flow(&escort(), competitive, Timers::default()).unwrap());
        run.flow.start();
        run.phase(100.0, &[], Phase::Round);
        assert_eq!(run.flow.clock(), start);
        // One pusher moves it at 1.5 m/s times Gibraltar's 1.1: the first
        // checkpoint, 20 m out, falls after 12.1 s and adds time.
        let mut elapsed = 0.0;
        let mut pushed = |run: &mut Run, until: FlowEvent| {
            let start = run.elapsed;
            loop {
                let position = run
                    .flow
                    .sided
                    .as_ref()
                    .map(|round| match &round.course {
                        Course::Payload(payload) => payload.position(),
                        _ => unreachable!(),
                    })
                    .unwrap();
                let events = run.flow.advance(STEP, &[at(0, 1, position)]);
                run.elapsed += STEP;
                elapsed += STEP;
                if events.contains(&until) {
                    return run.elapsed - start;
                }
                assert!(run.elapsed - start < 100.0);
            }
        };
        close(pushed(&mut run, FlowEvent::Objective(0)), 20.0 / 1.65);
        close(run.flow.clock(), start - 20.0 / 1.65 + bonus);
        // The checkpoint stops it for its 7.5 s pause.
        close(pushed(&mut run, FlowEvent::Objective(1)), 7.5 + 20.0 / 1.65);
        pushed(&mut run, FlowEvent::Phase(Phase::RoundEnd));
        assert!(elapsed > 30.0);
        if !competitive {
            assert_eq!(run.flow.standing(0, 1), Some(Standing::Won));
        }
    }
}

#[test]
fn hybrid_moves_its_payload_only_after_point_a() {
    let mut map = escort();
    map.mode_guid = HYBRID;
    map.objectives[0] = objective(ObjectiveKind::Point, 0, Some("Point A"), [0.0, 0.0, 0.0]);
    let mut run = Run::new(new_flow(&map, false, Timers::default()).unwrap());
    run.flow.start();
    run.phase(100.0, &[], Phase::Round);
    assert_eq!(run.flow.clock(), 300.0);
    let on_a = [at(0, 1, [0.0, 0.0, 0.0])];
    // The point's 32 s for one attacker.
    close(
        run.until(40.0, &on_a, |event| *event == FlowEvent::Objective(0))
            .unwrap(),
        32.0,
    );
    // The first payload leg adds its 3:00.
    close(run.flow.clock(), 300.0 - 32.0 + 180.0);
    assert_eq!(run.flow.objective_progress(), Some(0.0));
    // It waits 4 + 5 s once active.
    run.until(8.9, &on_a, |_| false);
    assert_eq!(run.flow.objective_progress(), Some(0.0));
    run.until(10.0, &on_a, |_| false);
    // It moves while he stays within 7 m of the point 2 m above it.
    let reach = (7.0f64 * 7.0 - 2.0 * 2.0).sqrt();
    let progress = run.flow.objective_progress().unwrap();
    assert!((progress - reach).abs() < 0.05, "{progress}");
}

fn control() -> GameMap {
    map(
        CONTROL,
        vec![
            objective(ObjectiveKind::ControlPoint, 0, Some("A"), A),
            objective(ObjectiveKind::ControlPoint, 1, Some("B"), B),
        ],
    )
}

#[test]
fn control_is_best_of_three_on_the_stages_in_turn() {
    let mut run = Run::new(new_flow(&control(), false, Timers::default()).unwrap());
    run.flow.start();
    close(
        run.phase(40.0, &[], Phase::Round),
        fixture(CONTROL).assemble,
    );
    assert_eq!(run.flow.clock(), 0.0, "no round clock");
    assert_eq!(
        run.flow.variables()[&OBJECTIVE_NAME],
        Value::String("A".into())
    );
    // Unlock 30 s, capture 8 s, 120 s of scoring.
    let took = run
        .until(200.0, &[at(0, 0, A)], |event| {
            *event == FlowEvent::RoundOver(Some(0))
        })
        .unwrap();
    assert!((took - 158.0).abs() < 0.5, "{took}");
    // Control waits 10.9 s after a round (0EC2), not Assault's 11.6 s.
    close(run.phase(20.0, &[], Phase::Round), VALUES.control_round_end);
    assert_eq!(
        run.flow.variables()[&OBJECTIVE_NAME],
        Value::String("B".into())
    );
    run.until(200.0, &[at(0, 0, B)], |event| {
        *event == FlowEvent::MatchOver
    })
    .unwrap();
    assert_eq!(run.flow.team_score(0), 2);
    assert_eq!(run.flow.standing(5, 0), Some(Standing::Won));
}

/// The flow's timers and goals are its rules' (the install's, read by
/// `mode_rules_data`), not constants: other rules play another match.
#[test]
fn the_flow_plays_by_the_rules_it_is_given() {
    let mut control_rules = fixture(CONTROL);
    control_rules.assemble = 10.0;
    control_rules.control_unlock = 5.0;
    control_rules.control.percent_per_second = 10.0;
    control_rules.score_to_win = 1;
    let flow = MatchFlow::new(
        &control(),
        false,
        VALUES,
        control_rules,
        objectives(&control()),
        Timers::default(),
    );
    let mut run = Run::new(flow.unwrap());
    run.flow.start();
    close(run.phase(40.0, &[], Phase::Round), 10.0);
    // Unlock 5 s, capture 8 s, 10 s of scoring; one round wins.
    let took = run
        .until(200.0, &[at(0, 0, A)], |event| {
            *event == FlowEvent::MatchOver
        })
        .unwrap();
    assert!((took - 23.0).abs() < 0.5, "{took}");

    let mut hybrid_rules = fixture(HYBRID);
    hybrid_rules.setup = 15.0;
    let flow = MatchFlow::new(
        &hybrid(),
        false,
        VALUES,
        hybrid_rules,
        objectives(&hybrid()),
        Timers::default(),
    );
    let mut run = Run::new(flow.unwrap());
    run.flow.start();
    close(run.phase(40.0, &[], Phase::Setup), 30.0);
    close(run.phase(60.0, &[], Phase::Round), 15.0);
}

/// Where the AI heroes go: Hybrid's point, then its payload, the moving
/// payload, the Control stage of the round (the next one's before it
/// starts); nothing in a mode without an objective.
#[test]
fn the_objective_follows_the_round() {
    let mut run = Run::new(new_flow(&hybrid(), false, Timers { scale: 0.1 }).unwrap());
    assert_eq!(run.flow.objective(), Some(A));
    run.flow.start();
    run.phase(100.0, &[], Phase::Round);
    run.until(100.0, &[at(0, 1, A)], |event| {
        *event == FlowEvent::Objective(0)
    })
    .unwrap();
    assert_eq!(run.flow.objective(), Some([0.0; 3]));

    let mut run = Run::new(new_flow(&escort(), false, Timers::default()).unwrap());
    run.flow.start();
    run.phase(100.0, &[], Phase::Round);
    assert_eq!(run.flow.objective(), Some([0.0; 3]));
    run.until(2.0, &[at(0, 1, [0.0; 3])], |_| false);
    assert!(run.flow.objective().unwrap()[0] > 3.0);

    let mut run = Run::new(new_flow(&control(), false, Timers::default()).unwrap());
    assert_eq!(run.flow.objective(), Some(A));
    run.flow.start();
    run.until(200.0, &[at(0, 0, A)], |event| {
        *event == FlowEvent::RoundOver(Some(0))
    })
    .unwrap();
    assert_eq!(run.flow.objective(), Some(A));
    run.phase(20.0, &[], Phase::Round);
    assert_eq!(run.flow.objective(), Some(B));

    let flow = new_flow(&map(DEATHMATCH, Vec::new()), false, Timers::default()).unwrap();
    assert_eq!(flow.objective(), None);
}

#[test]
fn control_overtime_holds_at_99_percent_while_contested() {
    let mut run = Run::new(new_flow(&control(), false, Timers::default()).unwrap());
    run.flow.start();
    run.phase(40.0, &[], Phase::Round);
    run.until(30.0 + 8.0 + 118.0, &[at(0, 0, A)], |_| false);
    let both = [at(0, 0, A), at(1, 1, A)];
    run.until(5.0, &both, |event| *event == FlowEvent::Overtime(true))
        .unwrap();
    assert!(run.until(30.0, &both, |_| true).is_none());
    assert_eq!(run.flow.control_percent(0), Some(99.0));
}

#[test]
fn deathmatch_scores_final_blows_minus_own_deaths_and_the_top_four_win() {
    let mut flow = new_flow(&map(DEATHMATCH, Vec::new()), false, Timers::default()).unwrap();
    flow.start();
    let players: Vec<_> = (0..8).map(|slot| at(slot, 0, SPAWN)).collect();
    let mut run = Run::new(flow);
    run.phase(40.0, &players, Phase::Round);
    assert_eq!(run.flow.clock(), fixture(DEATHMATCH).round_time);
    run.flow.kill(Some((2, 0)), (3, 0));
    run.flow.kill(None, (2, 0));
    run.flow.kill(Some((4, 0)), (4, 0));
    assert_eq!(run.flow.player_score(2), 0);
    assert_eq!(run.flow.player_score(4), -1);
    for _ in 0..fixture(DEATHMATCH).score_to_win {
        run.flow.kill(Some((5, 0)), (6, 0));
    }
    run.flow.kill(Some((1, 0)), (6, 0));
    run.until(1.0, &players, |event| *event == FlowEvent::MatchOver)
        .unwrap();
    let won: Vec<usize> = (0..8)
        .filter(|&slot| run.flow.standing(slot, 0) == Some(Standing::Won))
        .collect();
    // 5 (20), 1 (1), then 0 and 2 (0) by slot; 4 (-1) is out.
    assert_eq!(won, [0, 1, 2, 5]);
}

#[test]
fn team_deathmatch_wins_at_thirty_or_compares_at_the_time_limit() {
    let players = [at(0, 0, SPAWN), at(1, 1, SPAWN)];
    let mut run =
        Run::new(new_flow(&map(TEAM_DEATHMATCH, Vec::new()), false, Timers::default()).unwrap());
    run.flow.start();
    run.phase(40.0, &players, Phase::Round);
    for _ in 0..fixture(TEAM_DEATHMATCH).score_to_win {
        run.flow.kill(Some((1, 1)), (0, 0));
    }
    run.flow.kill(Some((0, 0)), (0, 0));
    assert_eq!(run.flow.team_score(0), 0, "no point for an own kill");
    run.until(1.0, &players, |event| *event == FlowEvent::MatchOver)
        .unwrap();
    assert_eq!(run.flow.standing(0, 1), Some(Standing::Won));

    let mut run =
        Run::new(new_flow(&map(TEAM_DEATHMATCH, Vec::new()), false, Timers::default()).unwrap());
    run.flow.start();
    run.phase(40.0, &players, Phase::Round);
    run.flow.kill(Some((0, 0)), (1, 1));
    run.flow.kill(Some((1, 1)), (0, 0));
    let ended = run
        .until(700.0, &players, |event| *event == FlowEvent::MatchOver)
        .unwrap();
    close(ended, fixture(TEAM_DEATHMATCH).round_time);
    assert_eq!(run.flow.standing(0, 0), Some(Standing::Drew));
}

#[test]
fn elimination_rounds_go_to_the_last_team_standing_first_to_three() {
    let mut run =
        Run::new(new_flow(&map(ELIMINATION, Vec::new()), false, Timers::default()).unwrap());
    run.flow.start();
    let alive = [at(0, 0, SPAWN), at(1, 1, SPAWN)];
    let red_out = [
        at(0, 0, SPAWN),
        Participant {
            slot: 1,
            team: 1,
            position: None,
        },
    ];
    for round in 0..3 {
        run.phase(40.0, &red_out, Phase::Round);
        // Dead at the round's start is not out yet: no one respawned.
        assert!(run.until(1.0, &red_out, |_| true).is_none());
        assert!(!run.flow.respawns(), "no respawn within a round");
        run.until(1.0, &alive, |_| false);
        run.until(1.0, &red_out, |event| {
            *event == FlowEvent::RoundOver(Some(0))
        })
        .unwrap();
        assert_eq!(run.flow.team_score(0), round + 1);
        if round < 2 {
            // Elimination waits 2.3 s after a round (0D0B).
            close(
                run.phase(20.0, &alive, Phase::Assemble),
                VALUES.elimination_round_end,
            );
        }
    }
    assert_eq!(run.flow.standing(0, 0), Some(Standing::Won));
}

#[test]
fn an_elimination_round_still_on_after_its_time_is_a_draw() {
    let mut run =
        Run::new(new_flow(&map(ELIMINATION, Vec::new()), false, Timers::default()).unwrap());
    run.flow.start();
    let alive = [at(0, 0, SPAWN), at(1, 1, SPAWN)];
    run.phase(40.0, &alive, Phase::Round);
    let ended = run
        .until(200.0, &alive, |event| *event == FlowEvent::RoundOver(None))
        .unwrap();
    close(ended, fixture(ELIMINATION).round_time);
    assert_eq!(run.flow.team_score(0) + run.flow.team_score(1), 0);
}

/// Overtime is what the bodies' `0C8E` reads as the mode's #3392e, which
/// adds 3 s to a respawn there (`WorldDelivery::set_overtime`).
#[test]
fn a_contested_end_starts_overtime() {
    let mut run = Run::new(new_flow(&hybrid(), false, Timers::default()).unwrap());
    run.flow.start();
    run.phase(100.0, &[], Phase::Round);
    assert!(!run.flow.in_overtime());
    run.until(400.0, &[at(0, 1, A), at(1, 0, A)], |event| {
        *event == FlowEvent::Overtime(true)
    })
    .unwrap();
    assert!(run.flow.in_overtime());
}

#[test]
fn spawn_rooms_follow_the_objective_and_swap_with_the_sides() {
    let mut run = Run::new(new_flow(&hybrid(), true, Timers::default()).unwrap());
    let room = |objective, attack| SpawnChoice::Objective { objective, attack };
    assert_eq!(run.flow.spawn_choice(1), room(0, true));
    assert_eq!(run.flow.spawn_choice(0), room(0, false));
    run.flow.start();
    run.phase(100.0, &[], Phase::Round);
    run.until(40.0, &[at(0, 1, A)], |event| {
        *event == FlowEvent::Objective(0)
    })
    .unwrap();
    // Rule 36: the attackers' spawn moves forward after A.
    assert_eq!(run.flow.spawn_choice(1), room(1, true));
    assert_eq!(run.flow.spawn_choice(0), room(1, false));
    run.phase(500.0, &[], Phase::RoundEnd);
    run.phase(20.0, &[], Phase::Assemble);
    // The sides swap: team 0 attacks from objective 0's attack rooms.
    assert_eq!(run.flow.spawn_choice(0), room(0, true));
    assert_eq!(run.flow.spawn_choice(1), room(0, false));

    // Escort: the checkpoint the payload heads for.
    let mut run = Run::new(new_flow(&escort(), false, Timers::default()).unwrap());
    run.flow.start();
    run.phase(100.0, &[], Phase::Round);
    assert_eq!(run.flow.spawn_choice(1), room(0, true));
    // An attacker walks along with the payload (its path runs along x).
    let mut reached = false;
    for _ in 0..(30.0 / STEP) as usize {
        let x = run.flow.objective_progress().unwrap();
        let events = run.flow.advance(STEP, &[at(0, 1, [x, 0.0, 0.0])]);
        if events.contains(&FlowEvent::Objective(0)) {
            reached = true;
            break;
        }
    }
    assert!(reached, "the first checkpoint");
    assert_eq!(run.flow.spawn_choice(0), room(1, false));

    // Control: the stage of the round (rule 60).
    let mut run = Run::new(new_flow(&control(), false, Timers::default()).unwrap());
    assert_eq!(run.flow.spawn_choice(0), SpawnChoice::Stage(0));
    run.flow.start();
    run.phase(40.0, &[], Phase::Round);
    run.until(200.0, &[at(0, 0, A)], |event| {
        *event == FlowEvent::RoundOver(Some(0))
    })
    .unwrap();
    run.phase(20.0, &[], Phase::Round);
    assert_eq!(run.flow.spawn_choice(1), SpawnChoice::Stage(1));
}

/// The attackers of `team`, `players` of them, walk with the payload until
/// `until` holds for an event; the time it took.
fn push(run: &mut Run, team: u8, players: usize, until: impl Fn(&FlowEvent) -> bool) -> f64 {
    let start = run.elapsed;
    loop {
        let position = run.flow.objective().expect("a payload");
        let on: Vec<_> = (0..players).map(|slot| at(slot, team, position)).collect();
        run.elapsed += STEP;
        let events = run.flow.advance(STEP, &on);
        let hit = events.iter().any(&until);
        run.events
            .extend(events.into_iter().map(|event| (run.elapsed, event)));
        if hit {
            return run.elapsed - start;
        }
        assert!(run.elapsed - start < 200.0, "pushing for 200 s");
    }
}

/// Both sides of a Competitive Escort deliver the payload: extra rounds on
/// the time banks (rules 67-69).
#[test]
fn competitive_escort_delivered_by_both_goes_to_extra_rounds_on_the_time_banks() {
    let mut run = Run::new(new_flow(&escort(), true, Timers::default()).unwrap());
    run.flow.start();
    // Three legs of 20 m: team 1 alone pushes at 1.5 × 1.1 m/s, team 0
    // with two at 1.75 × 1.1 m/s; the first checkpoint stops it 7.5 s. Of
    // 4:00 + 2:30 + 1:30 team 1 keeps 472.5 - 3 × 20 / 1.65 s, team 0
    // 472.5 - 3 × 20 / 1.925 s.
    for (team, players) in [(1, 1), (0, 2)] {
        run.phase(200.0, &[], Phase::Round);
        push(&mut run, team, players, |event| {
            *event == FlowEvent::Phase(Phase::RoundEnd)
        });
    }
    let (one, two) = (472.5 - 60.0 / 1.65, 472.5 - 60.0 / 1.925);
    assert!(run.flow.standing(0, 0).is_none(), "extra rounds, no result");
    assert!(
        !run.events
            .iter()
            .any(|(_, event)| matches!(event, FlowEvent::RoundOver(_)))
    );
    // Both over 2:00: cut by the same amount until the lower is 120 s; the
    // lower attacks first.
    run.phase(20.0, &[], Phase::Assemble);
    assert_eq!(run.flow.attackers(), Some(1));
    run.phase(100.0, &[], Phase::Round);
    close(run.flow.clock(), 120.0);
    // An extra round's checkpoint adds no time; then team 1 is stopped.
    push(&mut run, 1, 1, |event| *event == FlowEvent::Objective(0));
    close(run.flow.clock(), 120.0 - 20.0 / 1.65);
    run.phase(200.0, &[], Phase::RoundEnd);
    run.phase(20.0, &[], Phase::Assemble);
    assert_eq!(run.flow.attackers(), Some(0));
    run.phase(100.0, &[], Phase::Round);
    close(run.flow.clock(), 120.0 + two - one);
    // Team 0 reaches the second checkpoint: farther, it wins.
    push(&mut run, 0, 2, |event| *event == FlowEvent::Objective(1));
    run.until(300.0, &[], |event| *event == FlowEvent::MatchOver)
        .unwrap();
    assert_eq!(run.flow.standing(0, 0), Some(Standing::Won));
    assert_eq!(run.flow.standing(0, 1), Some(Standing::Lost));
}

#[test]
fn time_banks_are_cut_to_two_minutes_or_raised_to_one() {
    let banks = |mode, banks: [f64; 2]| {
        let mut map = if mode == HYBRID { hybrid() } else { escort() };
        map.mode_guid = mode;
        let mut flow = new_flow(&map, true, Timers::default()).unwrap();
        flow.upcoming.clear();
        flow.extra_rounds(banks, 1);
        (flow.single, flow.upcoming.into_iter().collect::<Vec<_>>())
    };
    // Rule 68: 4:37 and 2:50 become 3:47 and 2:00.
    assert_eq!(
        banks(ESCORT, [277.0, 170.0]),
        (false, vec![(1, 120.0), (0, 227.0)])
    );
    // Rule 67: 1:00 to 1:59 stay.
    assert_eq!(
        banks(ESCORT, [90.0, 70.0]),
        (false, vec![(1, 70.0), (0, 90.0)])
    );
    // Rule 67: under 1:00 the lower is raised to 1:00, the other alike.
    assert_eq!(
        banks(ESCORT, [30.0, 100.0]),
        (false, vec![(0, 60.0), (1, 130.0)])
    );
    // Rule 69: both at 0 s get 1:00; equal banks keep the first attackers.
    assert_eq!(
        banks(HYBRID, [0.0, 0.0]),
        (false, vec![(1, 60.0), (0, 60.0)])
    );
    // Rule 69: 0 s against 1:00 or more: only the other team attacks.
    assert_eq!(banks(HYBRID, [75.0, 0.0]), (true, vec![(0, 75.0)]));
    assert_eq!(
        banks(ESCORT, [75.0, 0.0]),
        (false, vec![(1, 60.0), (0, 135.0)])
    );
}

#[test]
fn a_lone_extra_attack_wins_with_a_tick_or_draws() {
    for (seconds, expected) in [(12.0, Standing::Won), (5.0, Standing::Drew)] {
        let mut flow = new_flow(&hybrid(), true, Timers::default()).unwrap();
        flow.upcoming.clear();
        flow.extra_rounds([75.0, 0.0], 1);
        flow.sided = None;
        flow.phase = Phase::RoundEnd;
        flow.left = 0.0;
        let mut run = Run::new(flow);
        run.phase(1.0, &[], Phase::Assemble);
        assert_eq!(run.flow.attackers(), Some(0));
        run.phase(100.0, &[], Phase::Round);
        close(run.flow.clock(), 75.0);
        // 12 s on A is a tick (rule 33), 5 s is none.
        run.until(seconds, &[at(0, 0, A)], |_| false);
        run.until(100.0, &[], |event| *event == FlowEvent::MatchOver)
            .unwrap();
        assert_eq!(run.flow.standing(0, 0), Some(expected), "{seconds} s");
    }
}

#[test]
fn an_elimination_point_unlocks_at_105_seconds_and_its_capture_wins() {
    let mut run =
        Run::new(new_flow(&map(ELIMINATION, Vec::new()), false, Timers::default()).unwrap());
    run.flow.start();
    // The fixture's one spawn point serves both teams: the point is there.
    let centre = GameMap::practice_range().spawns[0].position;
    let players = [at(0, 0, centre), at(1, 1, SPAWN)];
    run.phase(40.0, &players, Phase::Round);
    let won = run
        .until(200.0, &players, |event| {
            *event == FlowEvent::RoundOver(Some(0))
        })
        .unwrap();
    close(won, fixture(ELIMINATION).tiebreak + 3.0);
    assert_eq!(run.flow.team_score(0), 1);
    // Contested, it isn't taken: the round is a draw at 135 s.
    let contested = [at(0, 0, centre), at(1, 1, centre)];
    run.phase(40.0, &contested, Phase::Round);
    let drawn = run
        .until(200.0, &contested, |event| {
            matches!(event, FlowEvent::RoundOver(_))
        })
        .unwrap();
    close(drawn, fixture(ELIMINATION).round_time);
    assert!(
        run.events
            .iter()
            .any(|(_, event)| *event == FlowEvent::RoundOver(None))
    );
}
