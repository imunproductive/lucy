use super::*;
use orpheus_content::maps::MapObjective;

const STEP: f64 = 1.0 / 62.5;
const A: [f64; 3] = [100.0, 0.0, 0.0];
const B: [f64; 3] = [200.0, 0.0, 0.0];
const SPAWN: [f64; 3] = [0.0; 3];

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

fn assault() -> GameMap {
    map(
        ASSAULT,
        vec![
            objective(ObjectiveKind::Point, 1, Some("Point B"), B),
            objective(ObjectiveKind::Point, 0, Some("Point A"), A),
        ],
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
    assert!(MatchFlow::new(&GameMap::practice_range(), false, Timers::default()).is_none());
    assert!(MatchFlow::new(&map(ASSAULT, Vec::new()), false, Timers::default()).is_none());
    assert!(MatchFlow::new(&map(ESCORT, Vec::new()), false, Timers::default()).is_none());
    assert!(MatchFlow::new(&map(CONTROL, Vec::new()), false, Timers::default()).is_none());
    let skirmish = MatchFlow::new(&map(SKIRMISH, Vec::new()), false, Timers::default()).unwrap();
    assert_eq!(skirmish.mode(), Mode::Skirmish);
    assert_eq!(Mode::of(ASSAULT_TEST), Some(Mode::Assault));
    assert_eq!(Mode::of(BOUNTY_HUNTER), Some(Mode::Deathmatch));
}

#[test]
fn quick_play_assault_runs_its_phases_and_attackers_win_by_taking_a_and_b() {
    let mut run = Run::new(MatchFlow::new(&assault(), false, Timers::default()).unwrap());
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
    let defender = at(1, 0, B);
    close(run.phase(40.0, &[attacker, defender], Phase::Setup), 30.0);
    close(run.phase(60.0, &[attacker, defender], Phase::Round), 45.0);
    assert_eq!(run.flow.clock(), 240.0);
    assert!(!run.flow.assembling());
    // One attacker alone on A: 30 s (rule 33); 3:00 more (rule 31).
    let on_a = at(0, 1, A);
    let took = run
        .until(60.0, &[on_a, defender], |event| {
            *event == FlowEvent::Objective(0)
        })
        .unwrap();
    close(took, 30.0);
    close(run.flow.clock(), 240.0 - 30.0 + 180.0);
    assert_eq!(
        run.flow.variables()[&OBJECTIVE_NAME],
        Value::String("Point B".into())
    );
    // B with a defender on it holds; then three attackers take it in 10 s.
    let contested = [at(0, 1, B), defender];
    assert!(run.until(20.0, &contested, |_| true).is_none());
    assert_eq!(run.flow.objective_progress(), Some(0.0));
    let three = [at(0, 1, B), at(2, 1, B), at(3, 1, B), at(1, 0, SPAWN)];
    let took = run
        .until(20.0, &three, |event| *event == FlowEvent::MatchOver)
        .unwrap();
    close(took, 10.0);
    assert_eq!(run.flow.phase(), Phase::RoundEnd);
    assert_eq!(run.flow.standing(0, 1), Some(Standing::Won));
    assert_eq!(run.flow.standing(0, 0), Some(Standing::Lost));
    close(run.phase(20.0, &three, Phase::MatchEnd), ROUND_END);
    let finished = run
        .until(30.0, &three, |event| *event == FlowEvent::Finished)
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
    assert_eq!(winner, Some(Value::Integer(1)));
    assert!(!run.flow.respawns());
}

#[test]
fn the_time_scale_shortens_every_timer_and_capture() {
    let timers = Timers { scale: 0.1 };
    let mut run = Run::new(MatchFlow::new(&assault(), false, timers).unwrap());
    run.flow.start();
    let attacker = [at(0, 1, A)];
    close(run.phase(10.0, &attacker, Phase::Setup), 3.0);
    close(run.phase(10.0, &attacker, Phase::Round), 4.5);
    close(run.flow.clock(), 24.0);
    let took = run
        .until(10.0, &attacker, |event| *event == FlowEvent::Objective(0))
        .unwrap();
    close(took, 3.0);
}

#[test]
fn time_out_with_an_attacker_on_the_point_is_overtime_until_the_fuse_burns() {
    let mut run = Run::new(MatchFlow::new(&assault(), false, Timers::default()).unwrap());
    run.flow.start();
    let away = [at(0, 1, SPAWN)];
    run.phase(100.0, &away, Phase::Round);
    // An attacker contested by a defender when time runs out.
    let contested = [at(0, 1, A), at(1, 0, A)];
    let overtime = run
        .until(300.0, &contested, |event| {
            *event == FlowEvent::Overtime(true)
        })
        .unwrap();
    close(overtime, 240.0);
    assert!(run.flow.in_overtime());
    assert_eq!(run.flow.clock(), 0.0);
    // Held as long as he stays.
    assert!(run.until(25.0, &contested, |_| true).is_none());
    // He leaves after 25 s of overtime: the fuse is 3 s (rule 26).
    let ended = run
        .until(10.0, &away, |event| *event == FlowEvent::MatchOver)
        .unwrap();
    close(ended, 3.0);
    assert_eq!(
        run.flow.standing(0, 0),
        Some(Standing::Won),
        "defenders hold"
    );
}

#[test]
fn a_taken_in_overtime_goes_on_with_three_minutes() {
    let mut run = Run::new(MatchFlow::new(&assault(), false, Timers::default()).unwrap());
    run.flow.start();
    run.phase(100.0, &[], Phase::Round);
    // He reaches A with 10 s left and takes it alone in 30 s.
    run.until(230.0, &[at(0, 1, SPAWN)], |_| false);
    let taken = run
        .until(40.0, &[at(0, 1, A)], |event| {
            *event == FlowEvent::Objective(0)
        })
        .unwrap();
    close(taken, 30.0);
    assert!(!run.flow.in_overtime());
    close(run.flow.clock(), 180.0);
}

#[test]
fn competitive_assault_swaps_sides_and_the_farther_attack_wins() {
    let mut run = Run::new(MatchFlow::new(&assault(), true, Timers::default()).unwrap());
    run.flow.start();
    run.phase(100.0, &[], Phase::Round);
    // Team 1 takes A and a tick of B, then runs out of time.
    run.until(40.0, &[at(0, 1, A)], |event| {
        *event == FlowEvent::Objective(0)
    });
    run.until(7.0, &[at(0, 1, B)], |_| false);
    run.until(500.0, &[at(0, 1, SPAWN)], |event| {
        *event == FlowEvent::Phase(Phase::RoundEnd)
    })
    .unwrap();
    assert!(run.flow.standing(0, 0).is_none(), "not decided yet");
    // Hero select before the second round is 25 s (rule 2), then setup.
    close(run.phase(20.0, &[], Phase::Assemble), ROUND_END);
    assert_eq!(run.flow.attackers(), Some(0));
    close(run.phase(30.0, &[], Phase::Setup), 25.0);
    run.phase(50.0, &[], Phase::Round);
    // Team 0 takes A and less than a tick of B: team 1 wins.
    run.until(40.0, &[at(0, 0, A)], |event| {
        *event == FlowEvent::Objective(0)
    });
    run.until(3.0, &[at(0, 0, B)], |_| false);
    run.until(500.0, &[at(0, 0, SPAWN)], |event| {
        *event == FlowEvent::MatchOver
    })
    .unwrap();
    assert_eq!(run.flow.standing(3, 1), Some(Standing::Won));
    assert_eq!(run.flow.standing(3, 0), Some(Standing::Lost));
}

#[test]
fn competitive_assault_with_equal_progress_is_a_draw() {
    let mut run = Run::new(MatchFlow::new(&assault(), true, Timers::default()).unwrap());
    run.flow.start();
    for team in [1, 0] {
        run.phase(200.0, &[], Phase::Round);
        // Both get 20 s on A: one tick each (progress below a tick doesn't count).
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

#[test]
fn escort_checkpoints_add_time_and_the_delivery_wins() {
    let mut run = Run::new(MatchFlow::new(&escort(), false, Timers::default()).unwrap());
    run.flow.start();
    run.phase(100.0, &[], Phase::Round);
    assert_eq!(run.flow.clock(), ESCORT_TIME);
    // One pusher at the payload moves it at 1.7 m/s (rule 41): the first
    // checkpoint, 20 m out, falls after 11.8 s and adds time.
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
    close(pushed(&mut run, FlowEvent::Objective(0)), 20.0 / 1.7);
    close(
        run.flow.clock(),
        ESCORT_TIME - 20.0 / 1.7 + CHECKPOINT_BONUS[0],
    );
    close(pushed(&mut run, FlowEvent::Objective(1)), 20.0 / 1.7);
    pushed(&mut run, FlowEvent::MatchOver);
    assert_eq!(run.flow.standing(0, 1), Some(Standing::Won));
    assert!(elapsed > 30.0);
}

#[test]
fn hybrid_moves_its_payload_only_after_point_a() {
    let mut map = escort();
    map.mode_guid = HYBRID;
    map.objectives[0] = objective(ObjectiveKind::Point, 0, Some("Point A"), [0.0, 0.0, 0.0]);
    let mut run = Run::new(MatchFlow::new(&map, false, Timers::default()).unwrap());
    run.flow.start();
    run.phase(100.0, &[], Phase::Round);
    let on_a = [at(0, 1, [0.0, 0.0, 0.0])];
    // Rule 49: 32 s for one attacker.
    close(
        run.until(40.0, &on_a, |event| *event == FlowEvent::Objective(0))
            .unwrap(),
        32.0,
    );
    close(run.flow.clock(), ESCORT_TIME - 32.0 + HYBRID_BONUS);
    assert_eq!(run.flow.objective_progress(), Some(0.0));
    run.until(5.0, &on_a, |_| false);
    // It moves while he stays within its radius.
    close(
        run.flow.objective_progress().unwrap(),
        crate::objectives::PAYLOAD_RADIUS,
    );
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
    let mut run = Run::new(MatchFlow::new(&control(), false, Timers::default()).unwrap());
    run.flow.start();
    close(run.phase(40.0, &[], Phase::Round), ASSEMBLE);
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
    run.phase(20.0, &[], Phase::Round);
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

#[test]
fn control_overtime_holds_at_99_percent_while_contested() {
    let mut run = Run::new(MatchFlow::new(&control(), false, Timers::default()).unwrap());
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
    let mut flow = MatchFlow::new(&map(DEATHMATCH, Vec::new()), false, Timers::default()).unwrap();
    flow.start();
    let players: Vec<_> = (0..8).map(|slot| at(slot, 0, SPAWN)).collect();
    let mut run = Run::new(flow);
    run.phase(40.0, &players, Phase::Round);
    assert_eq!(run.flow.clock(), DEATHMATCH_TIME);
    run.flow.kill(Some((2, 0)), (3, 0));
    run.flow.kill(None, (2, 0));
    run.flow.kill(Some((4, 0)), (4, 0));
    assert_eq!(run.flow.player_score(2), 0);
    assert_eq!(run.flow.player_score(4), -1);
    for _ in 0..DEATHMATCH_SCORE {
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
    let mut run = Run::new(
        MatchFlow::new(&map(TEAM_DEATHMATCH, Vec::new()), false, Timers::default()).unwrap(),
    );
    run.flow.start();
    run.phase(40.0, &players, Phase::Round);
    for _ in 0..TEAM_DEATHMATCH_SCORE {
        run.flow.kill(Some((1, 1)), (0, 0));
    }
    run.flow.kill(Some((0, 0)), (0, 0));
    assert_eq!(run.flow.team_score(0), 0, "no point for an own kill");
    run.until(1.0, &players, |event| *event == FlowEvent::MatchOver)
        .unwrap();
    assert_eq!(run.flow.standing(0, 1), Some(Standing::Won));

    let mut run = Run::new(
        MatchFlow::new(&map(TEAM_DEATHMATCH, Vec::new()), false, Timers::default()).unwrap(),
    );
    run.flow.start();
    run.phase(40.0, &players, Phase::Round);
    run.flow.kill(Some((0, 0)), (1, 1));
    run.flow.kill(Some((1, 1)), (0, 0));
    let ended = run
        .until(700.0, &players, |event| *event == FlowEvent::MatchOver)
        .unwrap();
    close(ended, DEATHMATCH_TIME);
    assert_eq!(run.flow.standing(0, 0), Some(Standing::Drew));
}

#[test]
fn elimination_rounds_go_to_the_last_team_standing_first_to_three() {
    let mut run =
        Run::new(MatchFlow::new(&map(ELIMINATION, Vec::new()), false, Timers::default()).unwrap());
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
            close(run.phase(20.0, &alive, Phase::Assemble), ROUND_END);
        }
    }
    assert_eq!(run.flow.standing(0, 0), Some(Standing::Won));
}

#[test]
fn an_elimination_round_still_on_after_its_time_is_a_draw() {
    let mut run =
        Run::new(MatchFlow::new(&map(ELIMINATION, Vec::new()), false, Timers::default()).unwrap());
    run.flow.start();
    let alive = [at(0, 0, SPAWN), at(1, 1, SPAWN)];
    run.phase(40.0, &alive, Phase::Round);
    let ended = run
        .until(200.0, &alive, |event| *event == FlowEvent::RoundOver(None))
        .unwrap();
    close(ended, ELIMINATION_ROUND);
    assert_eq!(run.flow.team_score(0) + run.flow.team_score(1), 0);
}
